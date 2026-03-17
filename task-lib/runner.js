import { fork } from 'child_process';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { LOGS_DIR } from './config.js';
import { addTask, generateId, ensureDirs, loadTasks, saveTasks, updateTask } from './store.js';
import { createRequire } from 'module';

const require = createRequire(import.meta.url);
const { loadSettings } = require('../lib/settings.js');
const { normalizeProviderName } = require('../lib/provider-names');
const { getProvider } = require('../src/providers');

const __dirname = dirname(fileURLToPath(import.meta.url));
const TERMINAL_TASK_STATUSES = new Set(['completed', 'failed', 'stale', 'killed']);
const DEFAULT_RECONCILE_GRACE_MS = 15000;

export async function spawnTask(prompt, options = {}) {
  ensureDirs();
  await reconcileTasks();

  const id = generateId();
  const logFile = join(LOGS_DIR, `${id}.log`);
  const cwd = options.cwd || process.cwd();

  const settings = loadSettings();
  const { providerName, provider, providerSettings, levelOverrides } = resolveProviderContext(
    options,
    settings
  );

  const outputFormat = resolveOutputFormat(options);
  const jsonSchema = resolveJsonSchema(options, outputFormat);
  const modelSpec = resolveModelSpec(options, provider, providerSettings, levelOverrides);

  const cliFeatures = await provider.getCliFeatures();
  const commandSpec = provider.buildCommand(prompt, {
    modelSpec,
    outputFormat,
    jsonSchema,
    cwd,
    autoApprove: true,
    cliFeatures,
  });

  const finalArgs = resolveFinalArgs(commandSpec, providerName, options);
  const task = buildTaskRecord({
    id,
    prompt,
    cwd,
    options,
    logFile,
    providerName,
    modelSpec,
  });

  addTask(task);

  const watcherConfig = buildWatcherConfig(
    outputFormat,
    jsonSchema,
    options,
    providerName,
    commandSpec
  );
  const watcherScript = resolveWatcherScript({
    ...options,
    promptTransport: commandSpec.promptTransport || 'argv',
  });
  spawnWatcher({
    watcherScript,
    id,
    cwd,
    logFile,
    finalArgs,
    watcherConfig,
  });

  return task;
}

function resolveProviderContext(options, settings) {
  const providerName = normalizeProviderName(
    options.provider || settings.defaultProvider || 'claude'
  );
  const provider = getProvider(providerName);
  const providerSettings = settings.providerSettings?.[providerName] || {};
  const levelOverrides = providerSettings.levelOverrides || {};
  return { providerName, provider, providerSettings, levelOverrides };
}

function resolveOutputFormat(options) {
  return options.outputFormat || 'stream-json';
}

function resolveJsonSchema(options, outputFormat) {
  let jsonSchema = options.jsonSchema || null;
  if (jsonSchema && outputFormat !== 'json') {
    console.warn('Warning: --json-schema requires --output-format json, ignoring schema');
    jsonSchema = null;
  }
  return jsonSchema;
}

function resolveModelSpec(options, provider, providerSettings, levelOverrides) {
  if (options.model) {
    provider.validateModelId(options.model);
    return {
      model: options.model,
      reasoningEffort: options.reasoningEffort,
    };
  }

  const level = options.modelLevel || providerSettings.defaultLevel || provider.getDefaultLevel();
  let modelSpec = provider.resolveModelSpec(level, levelOverrides);
  if (options.reasoningEffort) {
    modelSpec = { ...modelSpec, reasoningEffort: options.reasoningEffort };
  }
  return modelSpec;
}

function resolveFinalArgs(commandSpec, providerName, options) {
  const finalArgs = [...commandSpec.args];
  if (providerName === 'claude') {
    const insertIndex =
      commandSpec.promptTransport === 'stdin' ? finalArgs.length : finalArgs.length - 1;
    if (options.resume) {
      finalArgs.splice(insertIndex, 0, '--resume', options.resume);
    } else if (options.continue) {
      finalArgs.splice(insertIndex, 0, '--continue');
    }
  } else if (options.resume || options.continue) {
    console.warn('Warning: resume/continue is only supported for Claude CLI; ignoring.');
  }
  return finalArgs;
}

function buildTaskRecord({ id, prompt, cwd, options, logFile, providerName, modelSpec }) {
  return {
    id,
    prompt: prompt.slice(0, 200) + (prompt.length > 200 ? '...' : ''),
    fullPrompt: prompt,
    cwd,
    status: 'running',
    pid: null,
    watcherPid: null,
    sessionId: options.resume || options.sessionId || null,
    logFile,
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
    exitCode: null,
    error: null,
    provider: providerName,
    model: modelSpec?.model || null,
    // Schedule reference (if spawned by scheduler)
    scheduleId: options.scheduleId || null,
    // Attach support
    socketPath: null,
    attachable: false,
  };
}

function buildWatcherConfig(outputFormat, jsonSchema, options, providerName, commandSpec) {
  return {
    outputFormat,
    jsonSchema,
    silentJsonOutput: options.silentJsonOutput || false,
    provider: providerName,
    command: commandSpec.binary,
    env: commandSpec.env || {},
    promptTransport: commandSpec.promptTransport || 'argv',
  };
}

function resolveWatcherScript(options) {
  const useAttachable =
    options.attachable !== false && !options.jsonSchema && options.promptTransport !== 'stdin';
  return useAttachable ? join(__dirname, 'attachable-watcher.js') : join(__dirname, 'watcher.js');
}

function spawnWatcher({ watcherScript, id, cwd, logFile, finalArgs, watcherConfig }) {
  const watcher = fork(
    watcherScript,
    [id, cwd, logFile, JSON.stringify(finalArgs), JSON.stringify(watcherConfig)],
    {
      detached: true,
      stdio: 'ignore',
    }
  );

  updateTask(id, { watcherPid: watcher.pid || null });
  watcher.unref();
  watcher.disconnect(); // Close IPC channel so parent can exit
}

function normalizePid(pid) {
  return Number.isInteger(pid) && pid > 0 ? pid : null;
}

export function isProcessRunning(pid) {
  if (!normalizePid(pid)) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

export function getTaskRuntimeState(task) {
  const childPid = normalizePid(task?.pid);
  const watcherPid = normalizePid(task?.watcherPid);
  const ownerPid = watcherPid ?? childPid;
  const ownerRunning = ownerPid !== null && isProcessRunning(ownerPid);
  const childRunning =
    childPid !== null ? (childPid === ownerPid ? ownerRunning : isProcessRunning(childPid)) : false;

  return {
    ownerPid,
    watcherPid,
    childPid,
    ownerRunning,
    childRunning,
    running: ownerRunning || childRunning,
  };
}

export function isTaskProcessRunning(task) {
  return getTaskRuntimeState(task).running;
}

function signalPid(pid, signal) {
  if (!normalizePid(pid)) return false;
  try {
    process.kill(pid, signal);
    return true;
  } catch {
    return false;
  }
}

function signalProcessGroup(groupLeaderPid, signal) {
  if (!normalizePid(groupLeaderPid)) return false;
  try {
    process.kill(-groupLeaderPid, signal);
    return true;
  } catch (error) {
    if (error && ['ESRCH', 'EINVAL', 'EPERM'].includes(error.code)) {
      return false;
    }
    throw error;
  }
}

function signalTaskRuntime(runtime, signal) {
  let signaled = false;

  if (runtime.ownerPid !== null) {
    signaled = signalProcessGroup(runtime.ownerPid, signal) || signaled;
    signaled = signalPid(runtime.ownerPid, signal) || signaled;
  }

  if (runtime.childPid !== null && runtime.childPid !== runtime.ownerPid) {
    signaled = signalPid(runtime.childPid, signal) || signaled;
  }

  return signaled;
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export async function terminateTask(taskOrPid, options = {}) {
  if (typeof taskOrPid === 'number') {
    return {
      signaled: signalPid(taskOrPid, 'SIGTERM'),
      exited: !isProcessRunning(taskOrPid),
      forced: false,
    };
  }

  const timeoutMs = options.timeoutMs ?? 1000;
  const forceKillTimeoutMs = options.forceKillTimeoutMs ?? 1000;
  const runtime = getTaskRuntimeState(taskOrPid);

  if (!runtime.running) {
    return { signaled: false, exited: true, forced: false };
  }

  const signaled = signalTaskRuntime(runtime, 'SIGTERM');
  if (!signaled) {
    return { signaled: false, exited: !getTaskRuntimeState(taskOrPid).running, forced: false };
  }

  const softDeadline = Date.now() + timeoutMs;
  while (Date.now() < softDeadline) {
    if (!getTaskRuntimeState(taskOrPid).running) {
      return { signaled: true, exited: true, forced: false };
    }
    await sleep(50);
  }

  signalTaskRuntime(runtime, 'SIGKILL');

  const hardDeadline = Date.now() + forceKillTimeoutMs;
  while (Date.now() < hardDeadline) {
    if (!getTaskRuntimeState(taskOrPid).running) {
      return { signaled: true, exited: true, forced: true };
    }
    await sleep(50);
  }

  return { signaled: true, exited: false, forced: true };
}

export async function reconcileTasks(options = {}) {
  const tasks = loadTasks();
  const reconcileGraceMs = options.runningTaskGraceMs ?? DEFAULT_RECONCILE_GRACE_MS;
  const report = { updated: [], reaped: [] };
  let changed = false;

  for (const task of Object.values(tasks)) {
    const runtime = getTaskRuntimeState(task);
    const nowIso = new Date().toISOString();
    const ageMs = Date.now() - new Date(task.updatedAt || task.createdAt || nowIso).getTime();

    if (TERMINAL_TASK_STATUSES.has(task.status)) {
      if (runtime.running) {
        const termination = await terminateTask(task);
        report.reaped.push({ id: task.id, exited: termination.exited });
      }

      if (!getTaskRuntimeState(task).running && (task.pid || task.watcherPid)) {
        task.pid = null;
        task.watcherPid = null;
        task.updatedAt = nowIso;
        changed = true;
        report.updated.push(task.id);
      }
      continue;
    }

    if (task.status !== 'running') {
      continue;
    }

    if (!runtime.running) {
      task.status = 'stale';
      task.error = task.error || 'Process died unexpectedly';
      task.pid = null;
      task.watcherPid = null;
      task.updatedAt = nowIso;
      changed = true;
      report.updated.push(task.id);
      continue;
    }

    if (runtime.watcherPid !== null && !runtime.ownerRunning && runtime.childRunning) {
      await terminateTask(task);
      task.status = 'stale';
      task.error = 'Watcher died unexpectedly; killed orphaned child process';
      task.pid = null;
      task.watcherPid = null;
      task.updatedAt = nowIso;
      changed = true;
      report.updated.push(task.id);
      continue;
    }

    if (
      runtime.watcherPid !== null &&
      runtime.ownerRunning &&
      !runtime.childRunning &&
      ageMs > reconcileGraceMs
    ) {
      await terminateTask(task);
      task.status = 'stale';
      task.error = 'Detached watcher outlived its child process';
      task.pid = null;
      task.watcherPid = null;
      task.updatedAt = nowIso;
      changed = true;
      report.updated.push(task.id);
    }
  }

  if (changed) {
    saveTasks(tasks);
  }

  return report;
}

export function killTask(taskOrPid) {
  if (typeof taskOrPid === 'number') {
    return signalPid(taskOrPid, 'SIGTERM');
  }

  return signalTaskRuntime(getTaskRuntimeState(taskOrPid), 'SIGTERM');
}
