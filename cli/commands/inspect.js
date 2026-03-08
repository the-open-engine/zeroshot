const fs = require('fs');
const chalk = require('chalk');
const Orchestrator = require('../../src/orchestrator');
const { detectIdType } = require('../../lib/id-detector');
const { getTask } = require('../../task-lib/store.js');
const { isProcessRunning } = require('../../task-lib/runner.js');
const {
  getProcessMetrics,
  isPlatformSupported: metricsPlatformSupported,
} = require('../../src/process-metrics');
const {
  analyzeProcessHealth,
  isPlatformSupported: stuckDetectorPlatformSupported,
} = require('../../src/agent/agent-stuck-detector');

const DEFAULT_SAMPLE_MS = 1000;
const TASK_STALE_WARNING_MS = 5 * 60 * 1000;

function parseSampleMs(rawValue) {
  const parsed = parseInt(rawValue ?? String(DEFAULT_SAMPLE_MS), 10);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    throw new Error(`Invalid sample period: ${rawValue}`);
  }
  return parsed;
}

function safeDate(dateString) {
  const date = new Date(dateString);
  return Number.isNaN(date.getTime()) ? null : date;
}

function formatAgeMs(ageMs) {
  if (!Number.isFinite(ageMs) || ageMs < 0) {
    return 'unknown';
  }

  const totalSeconds = Math.floor(ageMs / 1000);
  if (totalSeconds < 60) {
    return `${totalSeconds}s`;
  }

  const totalMinutes = Math.floor(totalSeconds / 60);
  if (totalMinutes < 60) {
    return `${totalMinutes}m`;
  }

  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  if (hours < 24) {
    return `${hours}h ${minutes}m`;
  }

  const days = Math.floor(hours / 24);
  const remainingHours = hours % 24;
  return `${days}d ${remainingHours}h`;
}

function inferActivity(metrics, health) {
  if (!metrics) {
    return 'unknown';
  }

  if (!metrics.exists) {
    return 'not-running';
  }

  if (health?.isLikelyStuck) {
    return 'likely-stuck';
  }

  if (metrics.cpuPercent >= 5) {
    return 'cpu-active';
  }

  if (metrics.network?.hasActivity) {
    return 'network-active';
  }

  if (metrics.childCount > 0) {
    return 'child-active';
  }

  if (metrics.state === 'R') {
    return 'running';
  }

  return 'idle';
}

function buildTaskWarnings(task, details) {
  const warnings = [];

  if (task.status === 'running' && task.pid && !details.processRunning) {
    warnings.push('task store says running but PID is not alive');
  }
  if (task.status === 'running' && task.logFile && !details.logFileExists) {
    warnings.push('task log file missing');
  }
  if (
    task.status === 'running' &&
    details.updatedAgeMs !== null &&
    details.updatedAgeMs > TASK_STALE_WARNING_MS
  ) {
    warnings.push(`task record stale for ${formatAgeMs(details.updatedAgeMs)}`);
  }

  return warnings;
}

function summarizeTaskRecord(task, now = Date.now(), existsSync = fs.existsSync) {
  if (!task) {
    return null;
  }

  const updatedAt = safeDate(task.updatedAt);
  const details = {
    updatedAgeMs: updatedAt ? now - updatedAt.getTime() : null,
    processRunning: task.pid ? isProcessRunning(task.pid) : false,
    logFileExists: Boolean(task.logFile && existsSync(task.logFile)),
    socketPathExists: Boolean(task.socketPath && existsSync(task.socketPath)),
  };
  const warnings = buildTaskWarnings(task, details);

  return {
    id: task.id,
    status: task.status,
    pid: task.pid || null,
    exitCode: task.exitCode ?? null,
    error: task.error || null,
    cwd: task.cwd || null,
    sessionId: task.sessionId || null,
    attachable: Boolean(task.attachable),
    socketPath: task.socketPath || null,
    socketPathExists: details.socketPathExists,
    logFile: task.logFile || null,
    logFileExists: details.logFileExists,
    createdAt: task.createdAt,
    updatedAt: task.updatedAt,
    updatedAgeMs: details.updatedAgeMs,
    updatedAgeHuman:
      details.updatedAgeMs === null ? 'unknown' : formatAgeMs(details.updatedAgeMs),
    processRunning: details.processRunning,
    warnings,
  };
}

async function inspectProcess(pid, sampleMs, options = {}) {
  if (!pid) {
    return null;
  }

  const includeHealth = options.includeHealth !== false;
  const metricsPromise = metricsPlatformSupported()
    ? getProcessMetrics(pid, { samplePeriodMs: sampleMs })
    : Promise.resolve(null);
  const healthPromise = includeHealth && stuckDetectorPlatformSupported()
    ? analyzeProcessHealth(pid, sampleMs)
    : Promise.resolve(null);

  const [metrics, health] = await Promise.all([metricsPromise, healthPromise]);
  return {
    pid,
    metrics,
    health,
    activity: inferActivity(metrics, health),
  };
}

async function inspectAgent(agent, options, deps = {}) {
  const getTaskImpl = deps.getTask || getTask;
  const existsSync = deps.existsSync || fs.existsSync;

  const process = await inspectProcess(agent.pid, options.sampleMs);
  const task = agent.currentTaskId
    ? summarizeTaskRecord(getTaskImpl(agent.currentTaskId), Date.now(), existsSync)
    : null;

  const warnings = [];
  if (agent.currentTask && !agent.currentTaskId) {
    warnings.push('agent reports running task but currentTaskId is missing');
  }

  return {
    ...agent,
    process,
    task,
    warnings,
  };
}

async function buildClusterInspection(clusterId, options = {}, deps = {}) {
  const sampleMs = parseSampleMs(options.sampleMs);
  const orchestrator =
    deps.orchestrator ||
    (deps.createOrchestrator
      ? await deps.createOrchestrator()
      : await Orchestrator.create({ quiet: true }));

  const status = deps.status || orchestrator.getStatus(clusterId);
  const clusterProcess = await inspectProcess(status.pid, sampleMs, { includeHealth: false });
  const agents = await Promise.all(
    status.agents.map((agent) => inspectAgent(agent, { sampleMs }, deps))
  );

  return {
    type: 'cluster',
    id: clusterId,
    sampleMs,
    inspectedAt: new Date().toISOString(),
    cluster: status,
    process: clusterProcess,
    agents,
  };
}

async function buildTaskInspection(taskId, options = {}, deps = {}) {
  const sampleMs = parseSampleMs(options.sampleMs);
  const getTaskImpl = deps.getTask || getTask;
  const existsSync = deps.existsSync || fs.existsSync;
  const task = getTaskImpl(taskId);

  if (!task) {
    throw new Error(`Task ${taskId} not found`);
  }

  return {
    type: 'task',
    id: taskId,
    sampleMs,
    inspectedAt: new Date().toISOString(),
    task: summarizeTaskRecord(task, Date.now(), existsSync),
    process: await inspectProcess(task.pid, sampleMs),
  };
}

function printProcessSection(label, processInfo, indent = '') {
  if (!processInfo) {
    console.log(`${indent}${chalk.dim(label)}: N/A`);
    return;
  }

  if (!processInfo.metrics?.exists) {
    console.log(`${indent}${chalk.dim(label)}: PID ${processInfo.pid} not running`);
    return;
  }

  const metrics = processInfo.metrics;
  console.log(
    `${indent}${chalk.dim(label)}: PID ${processInfo.pid} · activity ${processInfo.activity}`
  );
  console.log(
    `${indent}  state=${metrics.state} cpu=${metrics.cpuPercent}% mem=${metrics.memoryMB}MB threads=${metrics.threads} children=${metrics.childCount}`
  );

  const established = metrics.network?.established || 0;
  if (established > 0 || metrics.network?.hasActivity) {
    console.log(
      `${indent}  net=${established} conn sendQ=${metrics.network.sendQueueBytes} recvQ=${metrics.network.recvQueueBytes} activity=${metrics.network.hasActivity ? 'yes' : 'no'}`
    );
  }

  if (processInfo.health?.analysis) {
    console.log(`${indent}  health=${processInfo.health.analysis}`);
  }
}

function printWarnings(warnings, indent = '') {
  if (!warnings || warnings.length === 0) {
    return;
  }

  for (const warning of warnings) {
    console.log(`${indent}${chalk.yellow(`warning: ${warning}`)}`);
  }
}

function printTaskSection(task, indent = '') {
  if (!task) {
    return;
  }

  console.log(
    `${indent}${chalk.dim('Task')}: ${task.id} · ${task.status} · updated ${task.updatedAgeHuman} ago`
  );
  console.log(
    `${indent}  pid=${task.pid || 'N/A'} exit=${task.exitCode ?? 'N/A'} attachable=${task.attachable ? 'yes' : 'no'}`
  );
  console.log(
    `${indent}  log=${task.logFile || 'N/A'} (${task.logFileExists ? 'present' : 'missing'})`
  );
  if (task.socketPath) {
    console.log(
      `${indent}  socket=${task.socketPath} (${task.socketPathExists ? 'present' : 'missing'})`
    );
  }
  printWarnings(task.warnings, `${indent}  `);
}

function printClusterInspectionHuman(inspection) {
  console.log(`\nCluster Inspect: ${inspection.id}`);
  console.log(`State: ${inspection.cluster.state}`);
  console.log(`PID: ${inspection.cluster.pid || 'N/A'}`);
  console.log(`Created: ${new Date(inspection.cluster.createdAt).toLocaleString()}`);
  console.log(`Messages: ${inspection.cluster.messageCount}`);
  console.log(`Sample: ${inspection.sampleMs}ms`);

  console.log('\nCluster Process:');
  printProcessSection('process', inspection.process, '  ');

  console.log('\nAgents:');
  for (const agent of inspection.agents) {
    const modelLabel = agent.model ? ` [${agent.model}]` : '';
    console.log(`  - ${agent.id} (${agent.role})${modelLabel}`);
    console.log(
      `    state=${agent.state} iteration=${agent.iteration} runningTask=${agent.currentTask ? 'yes' : 'no'}`
    );
    if (agent.currentTaskId) {
      console.log(`    taskId=${agent.currentTaskId}`);
    }
    printProcessSection('process', agent.process, '    ');
    printTaskSection(agent.task, '    ');
    printWarnings(agent.warnings, '    ');
  }
  console.log('');
}

function printTaskInspectionHuman(inspection) {
  console.log(`\nTask Inspect: ${inspection.id}`);
  console.log(`Sample: ${inspection.sampleMs}ms`);
  printTaskSection(inspection.task);
  console.log('');
  printProcessSection('Process', inspection.process);
  console.log('');
}

async function runInspectCommand(id, options = {}, deps = {}) {
  const type = deps.detectIdType ? deps.detectIdType(id) : detectIdType(id);

  if (!type) {
    throw new Error(`ID not found: ${id}`);
  }

  const inspection =
    type === 'cluster'
      ? await buildClusterInspection(id, options, deps)
      : await buildTaskInspection(id, options, deps);

  if (options.json) {
    console.log(JSON.stringify(inspection, null, 2));
    return inspection;
  }

  if (inspection.type === 'cluster') {
    printClusterInspectionHuman(inspection);
  } else {
    printTaskInspectionHuman(inspection);
  }

  return inspection;
}

module.exports = {
  DEFAULT_SAMPLE_MS,
  TASK_STALE_WARNING_MS,
  buildClusterInspection,
  buildTaskInspection,
  inferActivity,
  parseSampleMs,
  runInspectCommand,
  summarizeTaskRecord,
};
