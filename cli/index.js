#!/usr/bin/env node

/**
 * zeroshot CLI
 *
 * Commands:
 * - run: Start a multi-agent cluster
 * - list: List all clusters and tasks
 * - status: Get cluster/task status
 * - logs: View cluster/task logs
 * - stop: Stop a cluster gracefully
 * - kill: Force kill a task or cluster
 * - kill-all: Kill all running tasks and clusters
 * - export: Export cluster conversation
 */

const { Command } = require('commander');
const path = require('path');
const fs = require('fs');
const os = require('os');
const chalk = require('chalk');
const Orchestrator = require('../src/orchestrator');
const { setupCompletion } = require('../lib/completion');
const { parseChunk } = require('../lib/stream-json-parser');
const { formatWatchMode } = require('./message-formatters-watch');
const {
  formatAgentLifecycle,
  formatAgentError: formatAgentErrorNormal,
  formatIssueOpened: formatIssueOpenedNormal,
  formatImplementationReady: formatImplementationReadyNormal,
  formatValidationResult: formatValidationResultNormal,
  formatPrCreated,
  formatClusterComplete,
  formatClusterFailed,
  formatGenericMessage,
} = require('./message-formatters-normal');
const {
  getColorForSender,
  buildMessagePrefix,
  buildClusterPrefix,
} = require('./message-formatter-utils');
const {
  loadSettings,
  saveSettings,
  validateSetting,
  coerceValue,
  DEFAULT_SETTINGS,
} = require('../lib/settings');
const { requirePreflight } = require('../src/preflight');
const { checkFirstRun } = require('./lib/first-run');
const { checkForUpdates } = require('./lib/update-checker');
const { StatusFooter } = require('../src/status-footer');

const program = new Command();

// =============================================================================
// GLOBAL ERROR HANDLERS - Prevent silent process death
// =============================================================================
// Track active cluster ID for cleanup on crash
/** @type {string | null} */
let activeClusterId = null;
/** @type {import('../src/orchestrator') | null} */
let orchestratorInstance = null;

// Track active status footer for safe output routing
// When set, all output routes through statusFooter.print() to prevent garbling
/** @type {import('../src/status-footer').StatusFooter | null} */
let activeStatusFooter = null;

/**
 * Safe print - routes through statusFooter when active to prevent garbling
 * @param {...any} args - Arguments to print (like console.log)
 */
function safePrint(...args) {
  const text = args.map(arg =>
    typeof arg === 'string' ? arg : String(arg)
  ).join(' ');

  if (activeStatusFooter) {
    activeStatusFooter.print(text + '\n');
  } else {
    console.log(...args);
  }
}

/**
 * Safe write - routes through statusFooter when active
 * @param {string} text - Text to write
 */
function safeWrite(text) {
  if (activeStatusFooter) {
    activeStatusFooter.print(text);
  } else {
    process.stdout.write(text);
  }
}

/**
 * Handle fatal errors: log, cleanup cluster state, exit
 * @param {string} type - 'uncaughtException' or 'unhandledRejection'
 * @param {Error|unknown} error - The error that caused the crash
 */
function handleFatalError(type, error) {
  const errorMessage = error instanceof Error ? error.message : String(error);
  const errorStack = error instanceof Error ? error.stack : '';

  console.error(chalk.red(`\n${'='.repeat(80)}`));
  console.error(chalk.red.bold(`🔴 FATAL: ${type}`));
  console.error(chalk.red(`${'='.repeat(80)}`));
  console.error(chalk.red(`Error: ${errorMessage}`));
  if (errorStack) {
    console.error(chalk.dim(errorStack));
  }
  console.error(chalk.red(`${'='.repeat(80)}\n`));

  // Try to update cluster state to 'failed' before exiting
  if (activeClusterId && orchestratorInstance) {
    try {
      console.error(chalk.yellow(`Attempting to mark cluster ${activeClusterId} as failed...`));
      const cluster = orchestratorInstance.clusters.get(activeClusterId);
      if (cluster) {
        cluster.state = 'failed';
        cluster.pid = null;
        cluster.failureInfo = {
          type,
          error: errorMessage,
          timestamp: Date.now(),
        };
        orchestratorInstance._saveClusters();
        console.error(chalk.yellow(`Cluster ${activeClusterId} marked as failed.`));
      }
    } catch (cleanupErr) {
      const errMsg = cleanupErr instanceof Error ? cleanupErr.message : String(cleanupErr);
      console.error(chalk.red(`Failed to update cluster state: ${errMsg}`));
    }
  }

  process.exit(1);
}

process.on('uncaughtException', (error) => {
  handleFatalError('Uncaught Exception', error);
});

process.on('unhandledRejection', (reason) => {
  handleFatalError('Unhandled Promise Rejection', reason);
});
// =============================================================================

// Package root directory (for resolving default config paths)
const PACKAGE_ROOT = path.resolve(__dirname, '..');

/**
 * Detect git repository root from current directory
 * Critical for CWD propagation - agents must work in the target repo, not where CLI was invoked
 * @returns {string} Git repo root, or process.cwd() if not in a git repo
 */
function detectGitRepoRoot() {
  const { execSync } = require('child_process');
  try {
    const root = execSync('git rev-parse --show-toplevel', {
      encoding: 'utf8',
      stdio: ['pipe', 'pipe', 'pipe'],
    }).trim();
    return root;
  } catch {
    // Not in a git repo - use current directory
    return process.cwd();
  }
}

// Lazy-loaded orchestrator (quiet by default) - created on first use
/** @type {import('../src/orchestrator') | null} */
let _orchestrator = null;
/**
 * @returns {import('../src/orchestrator')}
 */
function getOrchestrator() {
  if (!_orchestrator) {
    _orchestrator = new Orchestrator({ quiet: true });
  }
  return _orchestrator;
}

/**
 * @typedef {Object} TaskLogMessage
 * @property {string} topic
 * @property {string} sender
 * @property {Object} content
 * @property {number} timestamp
 */

/**
 * Read task logs from zeroshot task log files for agents in a cluster
 * Returns messages in cluster message format (topic, sender, content, timestamp)
 * @param {Object} cluster - Cluster object from orchestrator
 * @returns {TaskLogMessage[]} Messages from task logs
 */
function readAgentTaskLogs(cluster) {
  /** @type {TaskLogMessage[]} */
  const messages = [];
  const zeroshotLogsDir = path.join(os.homedir(), '.claude-zeroshot', 'logs');

  if (!fs.existsSync(zeroshotLogsDir)) {
    return messages;
  }

  // Strategy 1: Find task IDs from AGENT_LIFECYCLE messages
  const lifecycleMessages = cluster.messageBus.query({
    cluster_id: cluster.id,
    topic: 'AGENT_LIFECYCLE',
  });

  const taskIds = new Set(); // All task IDs we've found
  for (const msg of lifecycleMessages) {
    const taskId = msg.content?.data?.taskId;
    if (taskId) {
      taskIds.add(taskId);
    }
  }

  // Strategy 2: Find task IDs from current agent state
  for (const agent of cluster.agents) {
    const state = agent.getState();
    if (state.currentTaskId) {
      taskIds.add(state.currentTaskId);
    }
  }

  // Strategy 3: Scan for log files matching cluster start time (catch orphaned tasks)
  // This handles the case where TASK_ID_ASSIGNED wasn't published to cluster DB
  const clusterStartTime = cluster.createdAt;
  const logFiles = fs.readdirSync(zeroshotLogsDir);

  for (const logFile of logFiles) {
    if (!logFile.endsWith('.log')) continue;
    const taskId = logFile.replace(/\.log$/, '');

    // Check file modification time - only include logs modified after cluster started
    const logPath = path.join(zeroshotLogsDir, logFile);
    try {
      const stats = fs.statSync(logPath);
      if (stats.mtimeMs >= clusterStartTime) {
        taskIds.add(taskId);
      }
    } catch {
      // Skip files we can't stat
    }
  }

  // Read logs for all discovered tasks
  for (const taskId of taskIds) {
    const logPath = path.join(zeroshotLogsDir, `${taskId}.log`);
    if (!fs.existsSync(logPath)) {
      continue;
    }

    try {
      const content = fs.readFileSync(logPath, 'utf8');
      const lines = content.split('\n').filter((line) => line.trim());

      // Try to match task to agent (best effort, may not find a match for orphaned tasks)
      let matchedAgent = null;
      for (const agent of cluster.agents) {
        const state = agent.getState();
        if (state.currentTaskId === taskId) {
          matchedAgent = agent;
          break;
        }
      }

      // If no agent match, try to infer from lifecycle messages
      if (!matchedAgent) {
        for (const msg of lifecycleMessages) {
          if (msg.content?.data?.taskId === taskId) {
            const agentId = msg.content?.data?.agent || msg.sender;
            matchedAgent = cluster.agents.find((a) => a.id === agentId);
            break;
          }
        }
      }

      // Default to first agent if no match found (best effort for orphaned tasks)
      const agent = matchedAgent || cluster.agents[0];
      if (!agent) {
        continue;
      }

      const state = agent.getState();

      for (const line of lines) {
        // Lines are prefixed with [timestamp] - parse that first
        const trimmed = line.trim();
        if (!trimmed.startsWith('[')) {
          continue;
        }

        try {
          // Parse timestamp-prefixed line: [1733301234567]{json...} or [1733301234567][SYSTEM]...
          let timestamp = Date.now();
          let jsonContent = trimmed;

          const timestampMatch = jsonContent.match(/^\[(\d{13})\](.*)$/);
          if (timestampMatch) {
            timestamp = parseInt(timestampMatch[1], 10);
            jsonContent = timestampMatch[2];
          }

          // Skip non-JSON (e.g., [SYSTEM] lines)
          if (!jsonContent.startsWith('{')) {
            continue;
          }

          // Parse JSON
          const parsed = JSON.parse(jsonContent);

          // Skip system init messages
          if (parsed.type === 'system' && parsed.subtype === 'init') {
            continue;
          }

          // Convert to cluster message format
          messages.push({
            id: `task-${taskId}-${timestamp}`,
            timestamp,
            topic: 'AGENT_OUTPUT',
            sender: agent.id,
            receiver: 'broadcast',
            cluster_id: cluster.id,
            content: {
              text: jsonContent,
              data: {
                type: 'stdout',
                line: jsonContent,
                agent: agent.id,
                role: agent.role,
                iteration: state.iteration,
                fromTaskLog: true, // Mark as coming from task log
              },
            },
          });
        } catch {
          // Skip invalid JSON
        }
      }
    } catch (err) {
      // Log file read error - skip this task
      console.warn(`Warning: Could not read log for ${taskId}: ${err.message}`);
    }
  }

  return messages;
}

// Setup shell completion
setupCompletion();

// Banner disabled
function showBanner() {
  // Banner removed for cleaner output
}

// Show banner on startup (but not for completion, help, or daemon child)
const shouldShowBanner =
  !process.env.CREW_DAEMON &&
  !process.argv.includes('--completion') &&
  !process.argv.includes('-h') &&
  !process.argv.includes('--help') &&
  process.argv.length > 2;
if (shouldShowBanner) {
  showBanner();
}

// NOTE: Agent color handling moved to message-formatter-utils.js

program
  .name('zeroshot')
  .description('Multi-agent orchestration and task management for Claude')
  .version(require('../package.json').version)
  .option('-q, --quiet', 'Suppress prompts (first-run wizard, update checks)')
  .addHelpText(
    'after',
    `
Examples:
  ${chalk.cyan('zeroshot run 123 --ship')}             Full automation: isolated + auto-merge PR
  ${chalk.cyan('zeroshot run 123')}                    Run cluster and attach to first agent
  ${chalk.cyan('zeroshot run 123 -d')}                 Run cluster in background (detached)
  ${chalk.cyan('zeroshot run "Implement feature X"')}  Run cluster on plain text task
  ${chalk.cyan('zeroshot run 123 --isolation')}        Run in Docker container (safe for e2e tests)
  ${chalk.cyan('zeroshot task run "Fix the bug"')}     Run single-agent background task
  ${chalk.cyan('zeroshot list')}                       List all tasks and clusters
  ${chalk.cyan('zeroshot task list')}                  List tasks only
  ${chalk.cyan('zeroshot task watch')}                 Interactive TUI - navigate tasks, view logs
  ${chalk.cyan('zeroshot attach <id>')}                Attach to running task (Ctrl+B d to detach)
  ${chalk.cyan('zeroshot logs -f')}                    Stream logs in real-time (like tail -f)
  ${chalk.cyan('zeroshot logs -w')}                    Interactive watch mode (for tasks)
  ${chalk.cyan('zeroshot logs <id> -f')}               Stream logs for specific cluster/task
  ${chalk.cyan('zeroshot status <id>')}                Detailed status of task or cluster
  ${chalk.cyan('zeroshot finish <id>')}                Convert cluster to completion task (creates and merges PR)
  ${chalk.cyan('zeroshot kill <id>')}                  Kill a running task or cluster
  ${chalk.cyan('zeroshot purge')}                      Kill all processes and delete all data (with confirmation)
  ${chalk.cyan('zeroshot purge -y')}                   Purge everything without confirmation
  ${chalk.cyan('zeroshot settings')}                   Show/manage zeroshot settings (maxModel, config, etc.)
  ${chalk.cyan('zeroshot settings set <key> <val>')}   Set a setting (e.g., maxModel haiku)
  ${chalk.cyan('zeroshot config list')}                List available cluster configs
  ${chalk.cyan('zeroshot config show <name>')}         Visualize a cluster config (agents, triggers, flow)
  ${chalk.cyan('zeroshot export <id>')}                Export cluster conversation to file

Automation levels (cascading: --ship → --pr → --isolation):
  ${chalk.yellow('zeroshot run 123')}            → Local run, no isolation
  ${chalk.yellow('zeroshot run 123 --isolation')} → Docker isolation, no PR
  ${chalk.yellow('zeroshot run 123 --pr')}       → Isolation + PR (human reviews)
  ${chalk.yellow('zeroshot run 123 --ship')}     → Isolation + PR + auto-merge (full automation)
  ${chalk.yellow('zeroshot task run')}           → Single-agent background task (simpler, faster)

Shell completion:
  ${chalk.dim('zeroshot --completion >> ~/.bashrc && source ~/.bashrc')}
`
  );

// Run command - CLUSTER with auto-detection
program
  .command('run <input>')
  .description('Start a multi-agent cluster (auto-detects GitHub issue or plain text)')
  .option('--config <file>', 'Path to cluster config JSON (default: conductor-bootstrap)')
  .option('--isolation', 'Run cluster inside Docker container (for e2e testing)')
  .option(
    '--isolation-image <image>',
    'Docker image for isolation (default: zeroshot-cluster-base)'
  )
  .option(
    '--strict-schema',
    'Enforce JSON schema via CLI (no live streaming). Default: live streaming with local validation'
  )
  .option('--pr', 'Create PR for human review (auto-enables --isolation)')
  .option('--ship', 'Full automation: isolation + PR + auto-merge')
  .option('--workers <n>', 'Max sub-agents for worker to spawn in parallel', parseInt)
  .option('-d, --detach', 'Run in background (default: attach to first agent)')
  .addHelpText(
    'after',
    `
Input formats:
  123                              GitHub issue number (uses current repo)
  org/repo#123                     GitHub issue with explicit repo
  https://github.com/.../issues/1  Full GitHub issue URL
  "Implement feature X"            Plain text task description
`
  )
  .action(async (inputArg, options) => {
    try {
      // Cascading flag implications: --ship → --pr → --isolation
      // --ship = full automation (isolation + PR + auto-merge)
      if (options.ship) {
        options.pr = true;
        options.isolation = true;
      }
      // --pr = PR for human review (auto-enables isolation)
      if (options.pr) {
        options.isolation = true;
      }

      // Auto-detect input type
      let input = {};

      // Check if it's a GitHub issue URL
      if (inputArg.match(/^https?:\/\/github\.com\/[\w-]+\/[\w-]+\/issues\/\d+/)) {
        input.issue = inputArg;
      }
      // Check if it's a GitHub issue number (just digits)
      else if (/^\d+$/.test(inputArg)) {
        input.issue = inputArg;
      }
      // Check if it's org/repo#123 format
      else if (inputArg.match(/^[\w-]+\/[\w-]+#\d+$/)) {
        input.issue = inputArg;
      }
      // Otherwise, treat as plain text
      else {
        input.text = inputArg;
      }

      // === PREFLIGHT CHECKS ===
      // Validate all dependencies BEFORE starting anything
      // This gives users clear, actionable error messages upfront
      const preflightOptions = {
        requireGh: !!input.issue, // gh CLI required when fetching GitHub issues
        requireDocker: options.isolation, // Docker required for isolation mode
        quiet: process.env.CREW_DAEMON === '1', // Suppress success in daemon mode
      };
      requirePreflight(preflightOptions);

      // === CLUSTER MODE ===

      const { generateName } = require('../src/name-generator');

      // === DETACHED MODE (-d flag) ===
      // Spawn daemon and exit immediately
      if (options.detach && !process.env.CREW_DAEMON) {
        const { spawn } = require('child_process');

        // Generate cluster ID in parent so we can display it
        const clusterId = generateName('cluster');

        // Output cluster ID and help
        if (options.isolation) {
          console.log(`Started ${clusterId} (isolated)`);
        } else {
          console.log(`Started ${clusterId}`);
        }
        console.log(`Monitor: zeroshot logs ${clusterId} -f`);
        console.log(`Attach:  zeroshot attach ${clusterId}`);

        // Create log file for daemon output (captures startup errors)
        const osModule = require('os');
        const storageDir = path.join(osModule.homedir(), '.zeroshot');
        if (!fs.existsSync(storageDir)) {
          fs.mkdirSync(storageDir, { recursive: true });
        }
        const logPath = path.join(storageDir, `${clusterId}-daemon.log`);
        const logFd = fs.openSync(logPath, 'w');

        // Detect git repo root for CWD propagation
        // CRITICAL: Agents must work in the target repo, not where CLI was invoked
        const targetCwd = detectGitRepoRoot();

        // Spawn ourselves as daemon (detached, logs to file)
        const daemon = spawn(process.execPath, process.argv.slice(1), {
          detached: true,
          stdio: ['ignore', logFd, logFd], // stdout + stderr go to log file
          cwd: targetCwd, // Daemon inherits correct working directory
          env: {
            ...process.env,
            CREW_DAEMON: '1',
            CREW_CLUSTER_ID: clusterId,
            CREW_ISOLATION: options.isolation ? '1' : '',
            CREW_ISOLATION_IMAGE: options.isolationImage || '',
            CREW_PR: options.pr ? '1' : '',
            CREW_WORKERS: options.workers?.toString() || '',
            CREW_CWD: targetCwd, // Explicit CWD for orchestrator
          },
        });

        daemon.unref();
        fs.closeSync(logFd);
        process.exit(0);
      }

      // === FOREGROUND MODE (default) or DAEMON CHILD ===
      // Load user settings
      const settings = loadSettings();

      // Use cluster ID from env (daemon mode) or generate new one (foreground mode)
      // IMPORTANT: Set env var so orchestrator picks it up
      const clusterId = process.env.CREW_CLUSTER_ID || generateName('cluster');
      process.env.CREW_CLUSTER_ID = clusterId;

      // === LOAD CONFIG ===
      // Priority: CLI --config > settings.defaultConfig
      let config;
      const configName = options.config || settings.defaultConfig;

      // Resolve config path (check examples/ directory if not absolute/relative path)
      let configPath;
      if (
        path.isAbsolute(configName) ||
        configName.startsWith('./') ||
        configName.startsWith('../')
      ) {
        configPath = path.resolve(process.cwd(), configName);
      } else if (configName.endsWith('.json')) {
        // If it has .json extension, check examples/ directory
        configPath = path.join(PACKAGE_ROOT, 'cluster-templates', configName);
      } else {
        // Otherwise assume it's a template name (add .json)
        configPath = path.join(PACKAGE_ROOT, 'cluster-templates', `${configName}.json`);
      }

      // Create orchestrator with clusterId override for foreground mode
      const orchestrator = getOrchestrator();
      config = orchestrator.loadConfig(configPath);

      // Track for global error handler cleanup
      activeClusterId = clusterId;
      orchestratorInstance = orchestrator;

      // In foreground mode, show startup info
      if (!process.env.CREW_DAEMON) {
        if (options.isolation) {
          console.log(`Starting ${clusterId} (isolated)`);
        } else {
          console.log(`Starting ${clusterId}`);
        }
        console.log(chalk.dim(`Config: ${configName}`));
        console.log(chalk.dim('Ctrl+C to stop following (cluster keeps running)\n'));
      }

      // Apply strictSchema setting to all agents (CLI > env > settings)
      const strictSchema =
        options.strictSchema || process.env.CREW_STRICT_SCHEMA === '1' || settings.strictSchema;
      if (strictSchema) {
        for (const agent of config.agents) {
          agent.strictSchema = true;
        }
      }

      // Build start options (CLI flags > env vars > settings)
      // In foreground mode, use CLI options directly; in daemon mode, use env vars
      // CRITICAL: cwd must be passed to orchestrator for agent CWD propagation
      const targetCwd = process.env.CREW_CWD || detectGitRepoRoot();
      const startOptions = {
        cwd: targetCwd, // Target working directory for agents
        isolation:
          options.isolation || process.env.CREW_ISOLATION === '1' || settings.defaultIsolation,
        isolationImage: options.isolationImage || process.env.CREW_ISOLATION_IMAGE || undefined,
        autoPr: options.pr || process.env.CREW_PR === '1',
        autoMerge: process.env.CREW_MERGE === '1',
        autoPush: process.env.CREW_PUSH === '1',
      };

      // Start cluster
      const cluster = await orchestrator.start(config, input, startOptions);

      // === FOREGROUND MODE: Stream logs in real-time ===
      // Subscribe to message bus directly (same process) for instant output
      if (!process.env.CREW_DAEMON) {
        // Track senders that have output (for periodic flushing)
        const sendersWithOutput = new Set();
        // Track messages we've already processed (to avoid duplicates between history and subscription)
        const processedMessageIds = new Set();

        // === STATUS FOOTER: Live agent monitoring ===
        // Shows CPU, memory, network metrics for all agents at bottom of terminal
        const statusFooter = new StatusFooter({
          refreshInterval: 1000,
          enabled: process.stdout.isTTY,
        });
        statusFooter.setCluster(clusterId);
        statusFooter.setClusterState('running');
        statusFooter.setMessageBus(cluster.messageBus);
        // Set module-level reference so safePrint/safeWrite route through footer
        activeStatusFooter = statusFooter;

        // Subscribe to AGENT_LIFECYCLE to track agent states and PIDs
        const lifecycleUnsubscribe = cluster.messageBus.subscribeTopic('AGENT_LIFECYCLE', (msg) => {
          const data = msg.content?.data || {};
          const event = data.event;
          const agentId = data.agent || msg.sender;

          // Update agent state based on lifecycle event
          if (event === 'STARTED') {
            statusFooter.updateAgent({
              id: agentId,
              state: 'idle',
              pid: null,
              iteration: data.iteration || 0,
            });
          } else if (event === 'TASK_STARTED') {
            statusFooter.updateAgent({
              id: agentId,
              state: 'executing',
              pid: statusFooter.agents.get(agentId)?.pid || null,
              iteration: data.iteration || 0,
            });
          } else if (event === 'PROCESS_SPAWNED') {
            // Got the PID - update the agent with it
            const current = statusFooter.agents.get(agentId) || { state: 'executing', iteration: 0 };
            statusFooter.updateAgent({
              id: agentId,
              state: current.state,
              pid: data.pid,
              iteration: current.iteration,
            });
          } else if (event === 'TASK_COMPLETED' || event === 'TASK_FAILED') {
            statusFooter.updateAgent({
              id: agentId,
              state: 'idle',
              pid: null,
              iteration: data.iteration || 0,
            });
          } else if (event === 'STOPPED') {
            statusFooter.removeAgent(agentId);
          }
        });

        // Start the status footer
        statusFooter.start();

        // Message handler - processes messages, deduplicates by ID
        const handleMessage = (msg) => {
          if (msg.cluster_id !== clusterId) return;
          if (processedMessageIds.has(msg.id)) return;
          processedMessageIds.add(msg.id);

          if (msg.topic === 'AGENT_OUTPUT' && msg.sender) {
            sendersWithOutput.add(msg.sender);
          }
          printMessage(msg, false, false, true);
        };

        // Subscribe to NEW messages
        const unsubscribe = cluster.messageBus.subscribe(handleMessage);

        // CRITICAL: Replay historical messages that may have been published BEFORE we subscribed
        // This fixes the race condition where fast-completing clusters miss output
        const historicalMessages = cluster.messageBus.getAll(clusterId);
        for (const msg of historicalMessages) {
          handleMessage(msg);
        }

        // Periodic flush of text buffers (streaming text may not have newlines)
        const flushInterval = setInterval(() => {
          for (const sender of sendersWithOutput) {
            const prefix = getColorForSender(sender)(`${sender.padEnd(15)} |`);
            flushLineBuffer(prefix, sender);
          }
        }, 250);

        // Wait for cluster to complete
        await new Promise((resolve) => {
          const checkInterval = setInterval(() => {
            try {
              const status = orchestrator.getStatus(clusterId);
              if (status.state !== 'running') {
                clearInterval(checkInterval);
                clearInterval(flushInterval);
                lifecycleUnsubscribe();
                // Final flush BEFORE stopping status footer
                // (statusFooter.stop() sends ANSI codes that can clear terminal area)
                for (const sender of sendersWithOutput) {
                  const prefix = getColorForSender(sender)(`${sender.padEnd(15)} |`);
                  flushLineBuffer(prefix, sender);
                }
                // Stop status footer AFTER output is done
                statusFooter.stop();
                activeStatusFooter = null;
                unsubscribe();
                resolve();
              }
            } catch {
              // Cluster may have been removed
              clearInterval(checkInterval);
              clearInterval(flushInterval);
              statusFooter.stop();
              activeStatusFooter = null;
              lifecycleUnsubscribe();
              unsubscribe();
              resolve();
            }
          }, 500);

          // Handle Ctrl+C: Stop cluster since foreground mode has no daemon
          // CRITICAL: In foreground mode, the cluster runs IN this process.
          // If we exit without stopping, the cluster becomes a zombie (state=running but no process).
          process.on('SIGINT', async () => {
            // Stop status footer first to restore terminal
            statusFooter.stop();
            activeStatusFooter = null;
            lifecycleUnsubscribe();

            console.log(chalk.dim('\n\n--- Interrupted ---'));
            clearInterval(checkInterval);
            clearInterval(flushInterval);
            unsubscribe();

            // Stop the cluster properly so state is updated
            try {
              console.log(chalk.dim(`Stopping cluster ${clusterId}...`));
              await orchestrator.stop(clusterId);
              console.log(chalk.dim(`Cluster ${clusterId} stopped.`));
            } catch (stopErr) {
              console.error(chalk.red(`Failed to stop cluster: ${stopErr.message}`));
            }

            process.exit(0);
          });
        });

        console.log(chalk.dim(`\nCluster ${clusterId} completed.`));
      }

      // Daemon mode: cluster runs in background, stay alive via orchestrator's setInterval
      // Add cleanup handlers for daemon mode to ensure container cleanup on process exit
      // CRITICAL: Without this, containers become orphaned when daemon process dies
      if (process.env.CREW_DAEMON) {
        const cleanup = async (signal) => {
          console.log(`\n[DAEMON] Received ${signal}, cleaning up cluster ${clusterId}...`);
          try {
            await orchestrator.stop(clusterId);
            console.log(`[DAEMON] Cluster ${clusterId} stopped.`);
          } catch (e) {
            console.error(`[DAEMON] Cleanup error: ${e.message}`);
          }
          process.exit(0);
        };
        process.on('SIGTERM', () => cleanup('SIGTERM'));
        process.on('SIGINT', () => cleanup('SIGINT'));
      }
    } catch (error) {
      console.error('Error:', error.message);
      process.exit(1);
    }
  });

// === TASK COMMANDS ===
// Task run - single-agent background task
const taskCmd = program.command('task').description('Single-agent task management');

taskCmd
  .command('run <prompt>')
  .description('Run a single-agent background task')
  .option('-C, --cwd <path>', 'Working directory for task')
  .option('-r, --resume <sessionId>', 'Resume a specific Claude session')
  .option('-c, --continue', 'Continue the most recent session')
  .option(
    '-o, --output-format <format>',
    'Output format: stream-json (default), text, json',
    'stream-json'
  )
  .option('--json-schema <schema>', 'JSON schema for structured output')
  .option('--silent-json-output', 'Log ONLY final structured output')
  .action(async (prompt, options) => {
    try {
      // === PREFLIGHT CHECKS ===
      // Claude CLI must be installed and authenticated for task execution
      requirePreflight({
        requireGh: false, // gh not needed for plain tasks
        requireDocker: false, // Docker not needed for plain tasks
        quiet: false,
      });

      // Dynamically import task command (ESM module)
      const { runTask } = await import('../task-lib/commands/run.js');
      await runTask(prompt, options);
    } catch (error) {
      console.error('Error:', error.message);
      process.exit(1);
    }
  });

taskCmd
  .command('list')
  .alias('ls')
  .description('List all tasks (use "zeroshot list" to see both tasks and clusters)')
  .option('-s, --status <status>', 'Filter tasks by status (running, completed, failed)')
  .option('-n, --limit <n>', 'Limit number of results', parseInt)
  .option('-v, --verbose', 'Show detailed information (default: table view)')
  .action(async (options) => {
    try {
      // Get tasks only (dynamic import)
      const { listTasks } = await import('../task-lib/commands/list.js');
      await listTasks(options);
    } catch (error) {
      console.error('Error listing tasks:', error.message);
      process.exit(1);
    }
  });

taskCmd
  .command('watch')
  .description('Interactive TUI for tasks (navigate and view logs)')
  .option('--refresh-rate <ms>', 'Refresh interval in milliseconds', '1000')
  .action(async (options) => {
    try {
      const TaskTUI = (await import('../task-lib/tui.js')).default;
      const tui = new TaskTUI({
        refreshRate: parseInt(options.refreshRate, 10),
      });
      await tui.start();
    } catch (error) {
      console.error('Error starting task TUI:', error.message);
      console.error(error.stack);
      process.exit(1);
    }
  });

// List command - unified (shows both tasks and clusters)
program
  .command('list')
  .alias('ls')
  .description('List all tasks and clusters')
  .option('-s, --status <status>', 'Filter tasks by status (running, completed, failed)')
  .option('-n, --limit <n>', 'Limit number of results', parseInt)
  .option('--json', 'Output as JSON')
  .action(async (options) => {
    try {
      // Get clusters
      const clusters = getOrchestrator().listClusters();
      const orchestrator = getOrchestrator();

      // Enrich clusters with token data
      const enrichedClusters = clusters.map((cluster) => {
        let totalTokens = 0;
        let totalCostUsd = 0;
        try {
          const clusterObj = orchestrator.getCluster(cluster.id);
          if (clusterObj?.messageBus) {
            const tokensByRole = clusterObj.messageBus.getTokensByRole(cluster.id);
            if (tokensByRole?._total?.count > 0) {
              const total = tokensByRole._total;
              totalTokens = (total.inputTokens || 0) + (total.outputTokens || 0);
              totalCostUsd = total.totalCostUsd || 0;
            }
          }
        } catch {
          /* Token tracking not available */
        }
        return {
          ...cluster,
          totalTokens,
          totalCostUsd,
        };
      });

      // Get tasks (dynamic import)
      const { listTasks, getTasksData } = await import('../task-lib/commands/list.js');

      // JSON output mode
      if (options.json) {
        // Get tasks data if available
        let tasks = [];
        try {
          if (typeof getTasksData === 'function') {
            tasks = await getTasksData(options);
          }
        } catch {
          /* Tasks not available */
        }

        console.log(
          JSON.stringify(
            {
              clusters: enrichedClusters,
              tasks,
            },
            null,
            2
          )
        );
        return;
      }

      // Human-readable output (default)
      // Print clusters
      if (enrichedClusters.length > 0) {
        console.log(chalk.bold('\n=== Clusters ==='));
        console.log(
          `${'ID'.padEnd(25)} ${'State'.padEnd(12)} ${'Agents'.padEnd(8)} ${'Tokens'.padEnd(12)} ${'Cost'.padEnd(8)} Created`
        );
        console.log('-'.repeat(100));

        for (const cluster of enrichedClusters) {
          const created = new Date(cluster.createdAt).toLocaleString();
          const tokenDisplay = cluster.totalTokens > 0 ? cluster.totalTokens.toLocaleString() : '-';
          const costDisplay = cluster.totalCostUsd > 0 ? '$' + cluster.totalCostUsd.toFixed(3) : '-';

          // Highlight zombie clusters in red
          const stateDisplay =
            cluster.state === 'zombie'
              ? chalk.red(cluster.state.padEnd(12))
              : cluster.state.padEnd(12);

          const rowColor = cluster.state === 'zombie' ? chalk.red : (s) => s;

          console.log(
            `${rowColor(cluster.id.padEnd(25))} ${stateDisplay} ${cluster.agentCount.toString().padEnd(8)} ${tokenDisplay.padEnd(12)} ${costDisplay.padEnd(8)} ${created}`
          );
        }
      } else {
        console.log(chalk.dim('\n=== Clusters ==='));
        console.log('No active clusters');
      }

      // Print tasks
      console.log(chalk.bold('\n=== Tasks ==='));
      await listTasks(options);
    } catch (error) {
      console.error('Error listing:', error.message);
      process.exit(1);
    }
  });

// Status command - smart (works for both tasks and clusters)
program
  .command('status <id>')
  .description('Get detailed status of a task or cluster')
  .option('--json', 'Output as JSON')
  .action(async (id, options) => {
    try {
      const { detectIdType } = require('../lib/id-detector');
      const type = detectIdType(id);

      if (!type) {
        if (options.json) {
          console.log(JSON.stringify({ error: 'ID not found', id }, null, 2));
        } else {
          console.error(`ID not found: ${id}`);
          console.error('Not found in tasks or clusters');
        }
        process.exit(1);
      }

      if (type === 'cluster') {
        // Show cluster status
        const status = getOrchestrator().getStatus(id);

        // Get token usage
        let tokensByRole = null;
        try {
          const cluster = getOrchestrator().getCluster(id);
          if (cluster?.messageBus) {
            tokensByRole = cluster.messageBus.getTokensByRole(id);
          }
        } catch {
          /* Token tracking not available */
        }

        // JSON output mode
        if (options.json) {
          console.log(
            JSON.stringify(
              {
                type: 'cluster',
                ...status,
                createdAtISO: new Date(status.createdAt).toISOString(),
                tokensByRole,
              },
              null,
              2
            )
          );
          return;
        }

        // Human-readable output
        console.log(`\nCluster: ${status.id}`);
        if (status.isZombie) {
          console.log(
            chalk.red(
              `State: ${status.state} (process ${status.pid} died, cluster has no backing process)`
            )
          );
          console.log(
            chalk.yellow(
              `  → Run 'zeroshot kill ${id}' to clean up, or 'zeroshot resume ${id}' to restart`
            )
          );
        } else {
          console.log(`State: ${status.state}`);
        }
        if (status.pid) {
          console.log(`PID: ${status.pid}`);
        }
        console.log(`Created: ${new Date(status.createdAt).toLocaleString()}`);
        console.log(`Messages: ${status.messageCount}`);

        // Show token usage if available
        if (tokensByRole) {
          const tokenLines = formatTokenUsage(tokensByRole);
          if (tokenLines) {
            console.log('');
            for (const line of tokenLines) {
              console.log(line);
            }
          }
        }

        console.log(`\nAgents:`);

        for (const agent of status.agents) {
          // Check if subcluster
          if (agent.type === 'subcluster') {
            console.log(`  - ${agent.id} (${agent.role}) [SubCluster]`);
            console.log(`    State: ${agent.state}`);
            console.log(`    Iteration: ${agent.iteration}`);
            console.log(`    Child Cluster: ${agent.childClusterId || 'none'}`);
            console.log(`    Child Running: ${agent.childRunning ? 'Yes' : 'No'}`);
          } else {
            const modelLabel = agent.model ? ` [${agent.model}]` : '';
            console.log(`  - ${agent.id} (${agent.role})${modelLabel}`);
            console.log(`    State: ${agent.state}`);
            console.log(`    Iteration: ${agent.iteration}`);
            console.log(`    Running task: ${agent.currentTask ? 'Yes' : 'No'}`);
          }
        }

        console.log('');
      } else {
        // Show task status
        const { showStatus, getStatusData } = await import('../task-lib/commands/status.js');

        if (options.json) {
          // Try to get JSON data if available
          let taskData = null;
          try {
            if (typeof getStatusData === 'function') {
              taskData = await getStatusData(id);
            }
          } catch {
            /* Not available */
          }
          console.log(JSON.stringify({ type: 'task', id, ...taskData }, null, 2));
          return;
        }

        await showStatus(id);
      }
    } catch (error) {
      if (options.json) {
        console.log(JSON.stringify({ error: error.message }, null, 2));
      } else {
        console.error('Error getting status:', error.message);
      }
      process.exit(1);
    }
  });

// Logs command - smart (works for both tasks and clusters)
program
  .command('logs [id]')
  .description('View logs (omit ID for all clusters)')
  .option('-f, --follow', 'Follow logs in real-time (stream output like tail -f)')
  .option('-n, --limit <number>', 'Number of recent messages to show (default: 50)', '50')
  .option('--lines <number>', 'Number of lines to show (task mode)', parseInt)
  .option('-w, --watch', 'Watch mode: interactive TUI for tasks, high-level events for clusters')
  .action(async (id, options) => {
    try {
      // If ID provided, detect type
      if (id) {
        const { detectIdType } = require('../lib/id-detector');
        const type = detectIdType(id);

        if (!type) {
          console.error(`ID not found: ${id}`);
          process.exit(1);
        }

        if (type === 'task') {
          // Show task logs
          const { showLogs } = await import('../task-lib/commands/logs.js');
          await showLogs(id, options);
          return;
        }
        // Fall through to cluster logs below
      }

      // === CLUSTER LOGS ===
      const limit = parseInt(options.limit);
      const quietOrchestrator = new Orchestrator({ quiet: true });

      // No ID: show/follow ALL clusters
      if (!id) {
        const allClusters = quietOrchestrator.listClusters();
        const activeClusters = allClusters.filter((c) => c.state === 'running');

        if (allClusters.length === 0) {
          if (options.follow) {
            console.log('No clusters found. Waiting for new clusters...\n');
            console.log(chalk.dim('--- Waiting for clusters (Ctrl+C to stop) ---\n'));
          } else {
            console.log('No clusters found');
            return;
          }
        }

        // Track if multiple clusters
        const multiCluster = allClusters.length > 1;

        // Follow mode: show header
        if (options.follow && allClusters.length > 0) {
          if (activeClusters.length === 0) {
            console.log(
              chalk.dim(
                `--- Showing history from ${allClusters.length} cluster(s), waiting for new activity (Ctrl+C to stop) ---\n`
              )
            );
          } else if (activeClusters.length === 1) {
            console.log(chalk.dim(`--- Following ${activeClusters[0].id} (Ctrl+C to stop) ---\n`));
          } else {
            console.log(
              chalk.dim(
                `--- Following ${activeClusters.length} active clusters (Ctrl+C to stop) ---`
              )
            );
            for (const c of activeClusters) {
              console.log(chalk.dim(`    • ${c.id} [${c.state}]`));
            }
            console.log('');
          }
        }

        // Show recent messages from ALL clusters (history)
        // In follow mode, poll will handle new messages - this shows initial history
        for (const clusterInfo of allClusters) {
          const cluster = quietOrchestrator.getCluster(clusterInfo.id);
          if (cluster) {
            const messages = cluster.messageBus.getAll(clusterInfo.id);
            const recentMessages = messages.slice(-limit);
            const isActive = clusterInfo.state === 'running';
            for (const msg of recentMessages) {
              printMessage(msg, clusterInfo.id, options.watch, isActive);
            }
          }
        }

        // Follow mode: poll SQLite for new messages (cross-process support)
        if (options.follow) {
          // Set terminal title based on task(s)
          const taskTitles = [];
          for (const clusterInfo of allClusters) {
            const cluster = quietOrchestrator.getCluster(clusterInfo.id);
            if (cluster) {
              const messages = cluster.messageBus.getAll(clusterInfo.id);
              const issueOpened = messages.find((m) => m.topic === 'ISSUE_OPENED');
              if (issueOpened) {
                taskTitles.push({
                  id: clusterInfo.id,
                  summary: formatTaskSummary(issueOpened, 30),
                });
              }
            }
          }
          if (taskTitles.length === 1) {
            setTerminalTitle(`zeroshot [${taskTitles[0].id}]: ${taskTitles[0].summary}`);
          } else if (taskTitles.length > 1) {
            setTerminalTitle(`zeroshot: ${taskTitles.length} clusters`);
          } else {
            setTerminalTitle('zeroshot: waiting...');
          }

          // In watch mode, show the initial task for each cluster (after history)
          if (options.watch) {
            for (const clusterInfo of allClusters) {
              const cluster = quietOrchestrator.getCluster(clusterInfo.id);
              if (cluster) {
                const messages = cluster.messageBus.getAll(clusterInfo.id);
                const issueOpened = messages.find((m) => m.topic === 'ISSUE_OPENED');
                if (issueOpened) {
                  const clusterLabel = multiCluster ? `[${clusterInfo.id}] ` : '';
                  const taskSummary = formatTaskSummary(issueOpened);
                  console.log(chalk.cyan(`${clusterLabel}Task: ${chalk.bold(taskSummary)}\n`));
                }
              }
            }
          }

          const stopPollers = [];
          const messageBuffer = [];

          // Track cluster states (for dim coloring of inactive clusters)
          const clusterStates = new Map(); // cluster_id -> state
          for (const c of allClusters) {
            clusterStates.set(c.id, c.state);
          }

          // Track agent states from AGENT_LIFECYCLE messages (cross-process compatible)
          const agentStates = new Map(); // agent -> { state, timestamp }

          // Track if status line is currently displayed (to clear before printing logs)
          let statusLineShown = false;

          // Buffered message handler - collects messages and sorts by timestamp
          const flushMessages = () => {
            if (messageBuffer.length === 0) return;
            // Sort by timestamp
            messageBuffer.sort((a, b) => a.timestamp - b.timestamp);

            // Track senders with pending output
            const sendersWithOutput = new Set();
            for (const msg of messageBuffer) {
              if (msg.topic === 'AGENT_OUTPUT' && msg.sender) {
                sendersWithOutput.add(msg.sender);
              }
              // Track agent state from AGENT_LIFECYCLE messages
              if (msg.topic === 'AGENT_LIFECYCLE' && msg.sender && msg.content?.data?.state) {
                agentStates.set(msg.sender, {
                  state: msg.content.data.state,
                  model: msg.sender_model, // sender_model is always set by agent-wrapper._publish
                  timestamp: msg.timestamp || Date.now(),
                });
              }

              // Clear status line before printing message
              if (statusLineShown) {
                process.stdout.write('\r' + ' '.repeat(120) + '\r');
                statusLineShown = false;
              }

              const isActive = clusterStates.get(msg.cluster_id) === 'running';
              printMessage(msg, true, options.watch, isActive);
            }

            // Save cluster ID before clearing buffer
            const firstClusterId = messageBuffer[0]?.cluster_id;
            messageBuffer.length = 0;

            // Flush pending line buffers for all senders that had output
            // This ensures streaming text without newlines gets displayed
            for (const sender of sendersWithOutput) {
              const senderLabel = `${firstClusterId || ''}/${sender}`;
              const prefix = getColorForSender(sender)(`${senderLabel.padEnd(25)} |`);
              flushLineBuffer(prefix, sender);
            }
          };

          // Flush buffer every 250ms
          const flushInterval = setInterval(flushMessages, 250);

          // Blinking status indicator (follow/watch mode) - uses AGENT_LIFECYCLE state
          let blinkState = false;
          let statusInterval = null;
          if (options.follow || options.watch) {
            statusInterval = setInterval(() => {
              blinkState = !blinkState;

              // Get active agents from tracked states
              const activeList = [];
              for (const [agentId, info] of agentStates.entries()) {
                // Agent is active if not idle and not stopped
                if (info.state !== 'idle' && info.state !== 'stopped') {
                  activeList.push({
                    id: agentId,
                    state: info.state,
                    model: info.model,
                  });
                }
              }

              // Build status line - only show when agents are actively working
              if (activeList.length > 0) {
                const indicator = blinkState ? chalk.yellow('●') : chalk.dim('○');
                const agents = activeList
                  .map((a) => {
                    // Show state only for non-standard states (error, etc.)
                    const showState = a.state === 'error';
                    const stateLabel = showState ? chalk.red(` (${a.state})`) : '';
                    // Always show model
                    const modelLabel = a.model ? chalk.dim(` [${a.model}]`) : '';
                    return getColorForSender(a.id)(a.id) + modelLabel + stateLabel;
                  })
                  .join(', ');
                process.stdout.write(`\r${indicator} Active: ${agents}` + ' '.repeat(20));
                statusLineShown = true;
              } else {
                // Clear status line when no agents actively working
                if (statusLineShown) {
                  process.stdout.write('\r' + ' '.repeat(60) + '\r');
                  statusLineShown = false;
                }
              }
            }, 500);
          }

          for (const clusterInfo of allClusters) {
            const cluster = quietOrchestrator.getCluster(clusterInfo.id);
            if (cluster) {
              // Use polling for cross-process message detection
              const stopPoll = cluster.ledger.pollForMessages(
                clusterInfo.id,
                (msg) => {
                  messageBuffer.push(msg);
                },
                300
              );
              stopPollers.push(stopPoll);
            }
          }

          const stopWatching = quietOrchestrator.watchForNewClusters((newCluster) => {
            console.log(chalk.green(`\n✓ New cluster detected: ${newCluster.id}\n`));
            // Track new cluster as active
            clusterStates.set(newCluster.id, 'running');
            // Poll new cluster's ledger
            const stopPoll = newCluster.ledger.pollForMessages(
              newCluster.id,
              (msg) => {
                messageBuffer.push(msg);
              },
              300
            );
            stopPollers.push(stopPoll);
          });

          keepProcessAlive(() => {
            clearInterval(flushInterval);
            if (statusInterval) clearInterval(statusInterval);
            flushMessages();
            stopPollers.forEach((stop) => stop());
            stopWatching();
            // Clear status line on exit
            if (statusLineShown) {
              process.stdout.write('\r' + ' '.repeat(120) + '\r');
            }
            // Restore terminal title
            restoreTerminalTitle();
          });
        }
        return;
      }

      // Specific cluster ID provided
      const cluster = quietOrchestrator.getCluster(id);
      if (!cluster) {
        console.error(`Cluster ${id} not found`);
        process.exit(1);
      }

      // Check if cluster is active
      const allClustersList = quietOrchestrator.listClusters();
      const clusterInfo = allClustersList.find((c) => c.id === id);
      const isActive = clusterInfo?.state === 'running';

      // Get messages from cluster database
      const dbMessages = cluster.messageBus.getAll(id);

      // Get messages from agent task logs
      const taskLogMessages = readAgentTaskLogs(cluster);

      // Merge and sort by timestamp
      const allMessages = [...dbMessages, ...taskLogMessages].sort(
        (a, b) => a.timestamp - b.timestamp
      );
      const recentMessages = allMessages.slice(-limit);

      // Print messages
      for (const msg of recentMessages) {
        printMessage(msg, true, options.watch, isActive);
      }

      // Follow mode for specific cluster (poll SQLite AND task logs)
      if (options.follow) {
        // Set terminal title based on task
        const issueOpened = dbMessages.find((m) => m.topic === 'ISSUE_OPENED');
        if (issueOpened) {
          setTerminalTitle(`zeroshot [${id}]: ${formatTaskSummary(issueOpened, 30)}`);
        } else {
          setTerminalTitle(`zeroshot [${id}]`);
        }

        console.log('\n--- Following logs (Ctrl+C to stop) ---\n');

        // Poll cluster database for new messages
        const stopDbPoll = cluster.ledger.pollForMessages(
          id,
          (msg) => {
            printMessage(msg, true, options.watch, isActive);

            // Flush pending line buffer for streaming text without newlines
            if (msg.topic === 'AGENT_OUTPUT' && msg.sender) {
              const senderLabel = `${msg.cluster_id || ''}/${msg.sender}`;
              const prefix = getColorForSender(msg.sender)(`${senderLabel.padEnd(25)} |`);
              flushLineBuffer(prefix, msg.sender);
            }
          },
          500
        );

        // Poll agent task logs for new output
        const taskLogSizes = new Map(); // taskId -> last size
        const pollTaskLogs = () => {
          for (const agent of cluster.agents) {
            const state = agent.getState();
            const taskId = state.currentTaskId;
            if (!taskId) continue;

            const logPath = path.join(os.homedir(), '.claude-zeroshot', 'logs', `${taskId}.log`);
            if (!fs.existsSync(logPath)) continue;

            try {
              const stats = fs.statSync(logPath);
              const currentSize = stats.size;
              const lastSize = taskLogSizes.get(taskId) || 0;

              if (currentSize > lastSize) {
                // Read new content
                const fd = fs.openSync(logPath, 'r');
                const buffer = Buffer.alloc(currentSize - lastSize);
                fs.readSync(fd, buffer, 0, buffer.length, lastSize);
                fs.closeSync(fd);

                const newContent = buffer.toString('utf-8');
                const lines = newContent.split('\n').filter((line) => line.trim());

                for (const line of lines) {
                  if (!line.trim().startsWith('{')) continue;

                  try {
                    // Parse timestamp-prefixed line
                    let timestamp = Date.now();
                    let jsonContent = line.trim();

                    const timestampMatch = jsonContent.match(/^\[(\d{13})\](.*)$/);
                    if (timestampMatch) {
                      timestamp = parseInt(timestampMatch[1], 10);
                      jsonContent = timestampMatch[2];
                    }

                    if (!jsonContent.startsWith('{')) continue;

                    // Parse and validate JSON
                    const parsed = JSON.parse(jsonContent);
                    if (parsed.type === 'system' && parsed.subtype === 'init') continue;

                    // Create message and print immediately
                    const msg = {
                      id: `task-${taskId}-${timestamp}`,
                      timestamp,
                      topic: 'AGENT_OUTPUT',
                      sender: agent.id,
                      receiver: 'broadcast',
                      cluster_id: cluster.id,
                      content: {
                        text: jsonContent,
                        data: {
                          type: 'stdout',
                          line: jsonContent,
                          agent: agent.id,
                          role: agent.role,
                          iteration: state.iteration,
                          fromTaskLog: true,
                        },
                      },
                    };

                    printMessage(msg, true, options.watch, isActive);

                    // Flush line buffer
                    const senderLabel = `${cluster.id}/${agent.id}`;
                    const prefix = getColorForSender(agent.id)(`${senderLabel.padEnd(25)} |`);
                    flushLineBuffer(prefix, agent.id);
                  } catch {
                    // Skip invalid JSON
                  }
                }

                taskLogSizes.set(taskId, currentSize);
              }
            } catch {
              // File read error - skip
            }
          }
        };

        // Poll task logs every 300ms (same as agent-wrapper)
        const taskLogInterval = setInterval(pollTaskLogs, 300);

        keepProcessAlive(() => {
          stopDbPoll();
          clearInterval(taskLogInterval);
          restoreTerminalTitle();
        });
      }
    } catch (error) {
      console.error('Error viewing logs:', error.message);
      process.exit(1);
    }
  });

// Stop command (cluster-only)
program
  .command('stop <cluster-id>')
  .description('Stop a cluster gracefully')
  .action(async (clusterId) => {
    try {
      console.log(`Stopping cluster ${clusterId}...`);
      await getOrchestrator().stop(clusterId);
      console.log('Cluster stopped successfully');
    } catch (error) {
      console.error('Error stopping cluster:', error.message);
      process.exit(1);
    }
  });

// Kill command - smart (works for both tasks and clusters)
program
  .command('kill <id>')
  .description('Kill a task or cluster')
  .action(async (id) => {
    try {
      const { detectIdType } = require('../lib/id-detector');
      const type = detectIdType(id);

      if (!type) {
        console.error(`ID not found: ${id}`);
        process.exit(1);
      }

      if (type === 'cluster') {
        console.log(`Killing cluster ${id}...`);
        await getOrchestrator().kill(id);
        console.log('Cluster killed successfully');
      } else {
        // Kill task
        const { killTaskCommand } = await import('../task-lib/commands/kill.js');
        await killTaskCommand(id);
      }
    } catch (error) {
      console.error('Error killing:', error.message);
      process.exit(1);
    }
  });

// Attach command - tmux-style attach to running task or cluster agent
program
  .command('attach [id]')
  .description('Attach to a running task or cluster agent (Ctrl+C to detach, task keeps running)')
  .option('-a, --agent <name>', 'Attach to specific agent in cluster (required for clusters)')
  .addHelpText(
    'after',
    `
Examples:
  ${chalk.cyan('zeroshot attach')}                           List attachable tasks/clusters
  ${chalk.cyan('zeroshot attach task-xxx')}                  Attach to task
  ${chalk.cyan('zeroshot attach cluster-xxx --agent worker')} Attach to specific agent in cluster

Key bindings:
  ${chalk.yellow('Ctrl+C')}      Detach (task continues running)
  ${chalk.yellow('Ctrl+B d')}    Also detach (for tmux muscle memory)
  ${chalk.yellow('Ctrl+B ?')}    Show help
  ${chalk.yellow('Ctrl+B c')}    Interrupt agent (sends SIGINT) - USE WITH CAUTION
`
  )
  .action(async (id, options) => {
    try {
      const { AttachClient, socketDiscovery } = require('../src/attach');

      // If no ID provided, list attachable processes
      if (!id) {
        const tasks = await socketDiscovery.listAttachableTasks();
        const clusters = await socketDiscovery.listAttachableClusters();

        if (tasks.length === 0 && clusters.length === 0) {
          console.log(chalk.dim('No attachable tasks or clusters found.'));
          console.log(chalk.dim('Start a task with: zeroshot task run "prompt"'));
          return;
        }

        console.log(chalk.bold('\nAttachable processes:\n'));

        if (tasks.length > 0) {
          console.log(chalk.cyan('Tasks:'));
          for (const taskId of tasks) {
            console.log(`  ${taskId}`);
          }
        }

        if (clusters.length > 0) {
          console.log(chalk.yellow('\nClusters:'));
          const OrchestratorModule = require('../src/orchestrator');
          for (const clusterId of clusters) {
            const agents = await socketDiscovery.listAttachableAgents(clusterId);
            console.log(`  ${clusterId}`);
            // Get agent models and token usage from orchestrator (if available)
            let agentModels = {};
            let tokenUsageLines = null;
            try {
              const orchestrator = OrchestratorModule.getInstance();
              const status = orchestrator.getStatus(clusterId);
              for (const a of status.agents) {
                agentModels[a.id] = a.model;
              }
              // Get token usage from message bus
              const cluster = orchestrator.getCluster(clusterId);
              if (cluster?.messageBus) {
                const tokensByRole = cluster.messageBus.getTokensByRole(clusterId);
                tokenUsageLines = formatTokenUsage(tokensByRole);
              }
            } catch {
              /* orchestrator not running - models/tokens unavailable */
            }
            // Display token usage if available
            if (tokenUsageLines) {
              for (const line of tokenUsageLines) {
                console.log(`    ${line}`);
              }
            }
            for (const agent of agents) {
              const modelLabel = agentModels[agent] ? chalk.dim(` [${agentModels[agent]}]`) : '';
              console.log(chalk.dim(`    --agent ${agent}`) + modelLabel);
            }
          }
        }

        console.log(chalk.dim('\nUsage: zeroshot attach <id> [--agent <name>]'));
        return;
      }

      // Determine socket path
      let socketPath;

      if (id.startsWith('task-')) {
        socketPath = socketDiscovery.getTaskSocketPath(id);
      } else if (id.startsWith('cluster-')) {
        // Clusters use the task system - each agent spawns a task with its own socket
        // Get cluster status to find which task each agent is running
        const store = require('../lib/store');
        const cluster = store.getCluster(id);

        if (!cluster) {
          console.error(chalk.red(`Cluster ${id} not found`));
          process.exit(1);
        }

        if (cluster.state !== 'running') {
          console.error(chalk.red(`Cluster ${id} is not running (state: ${cluster.state})`));
          console.error(chalk.dim('Only running clusters have attachable agents.'));
          process.exit(1);
        }

        // Get orchestrator instance to query agent states
        const OrchestratorModule = require('../src/orchestrator');
        const orchestrator = OrchestratorModule.getInstance();

        try {
          const status = orchestrator.getStatus(id);
          const activeAgents = status.agents.filter(
            (a) => a.currentTaskId && a.state === 'executing_task'
          );

          if (activeAgents.length === 0) {
            console.error(chalk.yellow(`No agents currently executing tasks in cluster ${id}`));
            console.log(chalk.dim('\nAgent states:'));
            for (const agent of status.agents) {
              const modelLabel = agent.model ? chalk.dim(` [${agent.model}]`) : '';
              console.log(
                chalk.dim(
                  `  ${agent.id}${modelLabel}: ${agent.state}${agent.currentTaskId ? ` (last task: ${agent.currentTaskId})` : ''}`
                )
              );
            }
            return;
          }

          if (!options.agent) {
            // Show list of agents and their task IDs
            console.log(chalk.yellow(`\nCluster ${id} - attachable agents:\n`));
            for (const agent of activeAgents) {
              const modelLabel = agent.model ? chalk.dim(` [${agent.model}]`) : '';
              console.log(
                `  ${chalk.cyan(agent.id)}${modelLabel} → task ${chalk.green(agent.currentTaskId)}`
              );
              console.log(chalk.dim(`    zeroshot attach ${agent.currentTaskId}`));
            }
            console.log(chalk.dim('\nAttach to an agent by running: zeroshot attach <taskId>'));
            return;
          }

          // Find the specified agent
          const agent = status.agents.find((a) => a.id === options.agent);
          if (!agent) {
            console.error(chalk.red(`Agent '${options.agent}' not found in cluster ${id}`));
            console.log(
              chalk.dim('Available agents: ' + status.agents.map((a) => a.id).join(', '))
            );
            process.exit(1);
          }

          if (!agent.currentTaskId) {
            console.error(chalk.yellow(`Agent '${options.agent}' is not currently running a task`));
            console.log(chalk.dim(`State: ${agent.state}`));
            return;
          }

          // Use the agent's task socket
          socketPath = socketDiscovery.getTaskSocketPath(agent.currentTaskId);
          console.log(
            chalk.dim(`Attaching to agent ${options.agent} via task ${agent.currentTaskId}...`)
          );
        } catch (err) {
          // Orchestrator not running or cluster not loaded - fall back to socket discovery
          console.error(chalk.yellow(`Could not get cluster status: ${err.message}`));
          console.log(
            chalk.dim('Try attaching directly to a task ID instead: zeroshot attach <taskId>')
          );

          // Try to find any task sockets that might belong to this cluster
          const tasks = await socketDiscovery.listAttachableTasks();
          if (tasks.length > 0) {
            console.log(chalk.dim('\nAttachable tasks:'));
            for (const taskId of tasks) {
              console.log(chalk.dim(`  zeroshot attach ${taskId}`));
            }
          }
          return;
        }
      } else {
        // Try to auto-detect
        socketPath = socketDiscovery.getSocketPath(id, options.agent);
      }

      // Check if socket exists
      const socketAlive = await socketDiscovery.isSocketAlive(socketPath);
      if (!socketAlive) {
        console.error(chalk.red(`Cannot attach to ${id}`));

        // Check if it's an old task without attach support
        const { detectIdType } = require('../lib/id-detector');
        const type = detectIdType(id);

        if (type === 'task') {
          console.error(chalk.dim('Task may have been spawned before attach support was added.'));
          console.error(chalk.dim(`Try: zeroshot logs ${id} -f`));
        } else if (type === 'cluster') {
          console.error(chalk.dim('Cluster may not be running or agent may not exist.'));
          console.error(chalk.dim(`Check status: zeroshot status ${id}`));
        } else {
          console.error(chalk.dim('Process not found or not attachable.'));
        }
        process.exit(1);
      }

      // Connect
      console.log(
        chalk.dim(`Attaching to ${id}${options.agent ? ` (agent: ${options.agent})` : ''}...`)
      );
      console.log(chalk.dim('Press Ctrl+B ? for help, Ctrl+B d to detach\n'));

      const client = new AttachClient({ socketPath });

      client.on('state', (_state) => {
        // Could show status bar here in future
      });

      client.on('exit', ({ code, signal }) => {
        console.log(chalk.dim(`\n\nProcess exited (code: ${code}, signal: ${signal})`));
        process.exit(code || 0);
      });

      client.on('error', (err) => {
        console.error(chalk.red(`\nConnection error: ${err.message}`));
        process.exit(1);
      });

      client.on('detach', () => {
        console.log(chalk.dim('\n\nDetached. Task continues running.'));
        console.log(
          chalk.dim(
            `Re-attach: zeroshot attach ${id}${options.agent ? ` --agent ${options.agent}` : ''}`
          )
        );
        process.exit(0);
      });

      client.on('close', () => {
        console.log(chalk.dim('\n\nConnection closed.'));
        process.exit(0);
      });

      await client.connect();
    } catch (error) {
      console.error(chalk.red(`Error attaching: ${error.message}`));
      process.exit(1);
    }
  });

// Kill-all command - kills all running tasks and clusters
program
  .command('kill-all')
  .description('Kill all running tasks and clusters')
  .option('-y, --yes', 'Skip confirmation')
  .action(async (options) => {
    try {
      // Get counts first
      const orchestrator = getOrchestrator();
      const clusters = orchestrator.listClusters();
      const runningClusters = clusters.filter(
        (c) => c.state === 'running' || c.state === 'initializing'
      );

      const { loadTasks } = await import('../task-lib/store.js');
      const { isProcessRunning } = await import('../task-lib/runner.js');
      const tasks = loadTasks();
      const runningTasks = Object.values(tasks).filter(
        (t) => t.status === 'running' && isProcessRunning(t.pid)
      );

      const totalCount = runningClusters.length + runningTasks.length;

      if (totalCount === 0) {
        console.log(chalk.dim('No running tasks or clusters to kill.'));
        return;
      }

      // Show what will be killed
      console.log(chalk.bold(`\nWill kill:`));
      if (runningClusters.length > 0) {
        console.log(chalk.cyan(`  ${runningClusters.length} cluster(s)`));
        for (const c of runningClusters) {
          console.log(chalk.dim(`    - ${c.id}`));
        }
      }
      if (runningTasks.length > 0) {
        console.log(chalk.yellow(`  ${runningTasks.length} task(s)`));
        for (const t of runningTasks) {
          console.log(chalk.dim(`    - ${t.id}`));
        }
      }

      // Confirm unless -y flag
      if (!options.yes) {
        const readline = require('readline');
        const rl = readline.createInterface({
          input: process.stdin,
          output: process.stdout,
        });

        const answer = await new Promise((resolve) => {
          rl.question(chalk.bold('\nProceed? [y/N] '), resolve);
        });
        rl.close();

        if (answer.toLowerCase() !== 'y') {
          console.log('Aborted.');
          return;
        }
      }

      console.log('');

      // Kill clusters
      if (runningClusters.length > 0) {
        const clusterResults = await orchestrator.killAll();
        for (const id of clusterResults.killed) {
          console.log(chalk.green(`✓ Killed cluster: ${id}`));
        }
        for (const err of clusterResults.errors) {
          console.log(chalk.red(`✗ Failed to kill cluster ${err.id}: ${err.error}`));
        }
      }

      // Kill tasks
      if (runningTasks.length > 0) {
        const { killTask, isProcessRunning: checkPid } = await import('../task-lib/runner.js');
        const { updateTask } = await import('../task-lib/store.js');

        for (const task of runningTasks) {
          if (!checkPid(task.pid)) {
            updateTask(task.id, {
              status: 'stale',
              error: 'Process died unexpectedly',
            });
            console.log(chalk.yellow(`○ Task ${task.id} was already dead, marked stale`));
            continue;
          }

          const killed = killTask(task.pid);
          if (killed) {
            updateTask(task.id, {
              status: 'killed',
              error: 'Killed by kill-all',
            });
            console.log(chalk.green(`✓ Killed task: ${task.id}`));
          } else {
            console.log(chalk.red(`✗ Failed to kill task: ${task.id}`));
          }
        }
      }

      console.log(chalk.bold.green(`\nDone.`));
    } catch (error) {
      console.error('Error:', error.message);
      process.exit(1);
    }
  });

// Export command (cluster-only)
program
  .command('export <cluster-id>')
  .description('Export cluster conversation')
  .option('-f, --format <format>', 'Export format: json, markdown, pdf', 'pdf')
  .option('-o, --output <file>', 'Output file (auto-generated for pdf)')
  .action(async (clusterId, options) => {
    try {
      // Get messages from DB
      const Ledger = require('../src/ledger');
      const homeDir = require('os').homedir();
      const dbPath = path.join(homeDir, '.zeroshot', `${clusterId}.db`);

      if (!require('fs').existsSync(dbPath)) {
        throw new Error(`Cluster ${clusterId} not found (no DB file)`);
      }

      const ledger = new Ledger(dbPath);
      const messages = ledger.getAll(clusterId);
      ledger.close();

      // JSON export
      if (options.format === 'json') {
        const data = JSON.stringify({ cluster_id: clusterId, messages }, null, 2);
        if (options.output) {
          require('fs').writeFileSync(options.output, data, 'utf8');
          console.log(`Exported to ${options.output}`);
        } else {
          console.log(data);
        }
        return;
      }

      // Terminal-style export (for markdown and pdf)
      const terminalOutput = renderMessagesToTerminal(clusterId, messages);

      if (options.format === 'markdown') {
        // Strip ANSI codes for markdown
        const plainText = terminalOutput.replace(/\\x1b\[[0-9;]*m/g, '');
        if (options.output) {
          require('fs').writeFileSync(options.output, plainText, 'utf8');
          console.log(`Exported to ${options.output}`);
        } else {
          console.log(plainText);
        }
        return;
      }

      // PDF export - convert ANSI to HTML, then to PDF
      const outputFile = options.output || `${clusterId}.pdf`;
      const AnsiToHtml = require('ansi-to-html');
      const { mdToPdf } = await import('md-to-pdf');

      const ansiConverter = new AnsiToHtml({
        fg: '#d4d4d4',
        bg: '#1e1e1e',
        colors: {
          0: '#1e1e1e',
          1: '#f44747',
          2: '#6a9955',
          3: '#dcdcaa',
          4: '#569cd6',
          5: '#c586c0',
          6: '#4ec9b0',
          7: '#d4d4d4',
          8: '#808080',
          9: '#f44747',
          10: '#6a9955',
          11: '#dcdcaa',
          12: '#569cd6',
          13: '#c586c0',
          14: '#4ec9b0',
          15: '#ffffff',
        },
      });

      const htmlContent = ansiConverter.toHtml(terminalOutput);
      const fullHtml = `<pre style="margin:0;padding:0;white-space:pre-wrap;word-wrap:break-word;">${htmlContent}</pre>`;

      const pdf = await mdToPdf(
        { content: fullHtml },
        {
          pdf_options: {
            format: 'A4',
            margin: {
              top: '10mm',
              right: '10mm',
              bottom: '10mm',
              left: '10mm',
            },
            printBackground: true,
          },
          css: `
            @page { size: A4 landscape; }
            body {
              margin: 0; padding: 16px;
              background: #1e1e1e; color: #d4d4d4;
              font-family: 'JetBrains Mono', 'Fira Code', 'Consolas', 'Monaco', monospace;
              font-size: 9pt; line-height: 1.4;
            }
            pre { margin: 0; font-family: inherit; }
          `,
        }
      );

      require('fs').writeFileSync(outputFile, pdf.content);
      console.log(`Exported to ${outputFile}`);
    } catch (error) {
      console.error('Error exporting cluster:', error.message);
      process.exit(1);
    }
  });

// === TASK-SPECIFIC COMMANDS ===

// Resume task or cluster
program
  .command('resume <id> [prompt]')
  .description('Resume a failed task or cluster')
  .option('-d, --detach', 'Resume in background (daemon mode)')
  .action(async (id, prompt, options) => {
    try {
      // Try cluster first, then task (both use same ID format: "adjective-noun-number")
      const OrchestratorModule = require('../src/orchestrator');
      const orchestrator = new OrchestratorModule();

      // Check if cluster exists
      const cluster = orchestrator.getCluster(id);

      // === PREFLIGHT CHECKS ===
      // Claude CLI must be installed and authenticated
      // Check if cluster uses isolation (needs Docker)
      const requiresDocker = cluster?.isolation?.enabled || false;
      requirePreflight({
        requireGh: false, // Resume doesn't fetch new issues
        requireDocker: requiresDocker,
        quiet: false,
      });

      if (cluster) {
        // Resume cluster
        console.log(chalk.cyan(`Resuming cluster ${id}...`));
        const result = await orchestrator.resume(id, prompt);

        console.log(chalk.green(`✓ Cluster resumed`));
        if (result.resumeType === 'failure') {
          console.log(`  Resume type: ${chalk.yellow('From failure')}`);
          console.log(`  Resumed agent: ${result.resumedAgent}`);
          console.log(`  Previous error: ${result.previousError}`);
        } else {
          console.log(`  Resume type: ${chalk.cyan('Clean continuation')}`);
          if (result.resumedAgents && result.resumedAgents.length > 0) {
            console.log(`  Resumed agents: ${result.resumedAgents.join(', ')}`);
          } else {
            console.log(`  Published CLUSTER_RESUMED to trigger workflow`);
          }
        }

        // === DAEMON MODE: Exit and let cluster run in background ===
        if (options.detach) {
          console.log('');
          console.log(chalk.dim(`Follow logs with: zeroshot logs ${id} -f`));
          return;
        }

        // === FOREGROUND MODE: Stream logs in real-time (same as 'run' command) ===
        console.log('');
        console.log(chalk.dim('Streaming logs... (Ctrl+C to stop cluster)'));
        console.log('');

        // Get the cluster's message bus for streaming
        const resumedCluster = orchestrator.getCluster(id);
        if (!resumedCluster || !resumedCluster.messageBus) {
          console.error(chalk.red('Failed to get message bus for resumed cluster'));
          process.exit(1);
        }

        // Track senders that have output (for periodic flushing)
        const sendersWithOutput = new Set();
        // Track messages we've already processed (to avoid duplicates between history and subscription)
        const processedMessageIds = new Set();

        // Message handler - processes messages, deduplicates by ID
        const handleMessage = (msg) => {
          if (msg.cluster_id !== id) return;
          if (processedMessageIds.has(msg.id)) return;
          processedMessageIds.add(msg.id);

          if (msg.topic === 'AGENT_OUTPUT' && msg.sender) {
            sendersWithOutput.add(msg.sender);
          }
          printMessage(msg, false, false, true);
        };

        // Subscribe to NEW messages
        const unsubscribe = resumedCluster.messageBus.subscribe(handleMessage);

        // Periodic flush of text buffers (streaming text may not have newlines)
        const flushInterval = setInterval(() => {
          for (const sender of sendersWithOutput) {
            const prefix = getColorForSender(sender)(`${sender.padEnd(15)} |`);
            flushLineBuffer(prefix, sender);
          }
        }, 250);

        // Wait for cluster to complete
        await new Promise((resolve) => {
          const checkInterval = setInterval(() => {
            try {
              const status = orchestrator.getStatus(id);
              if (status.state !== 'running') {
                clearInterval(checkInterval);
                clearInterval(flushInterval);
                // Final flush
                for (const sender of sendersWithOutput) {
                  const prefix = getColorForSender(sender)(`${sender.padEnd(15)} |`);
                  flushLineBuffer(prefix, sender);
                }
                unsubscribe();
                resolve();
              }
            } catch {
              // Cluster may have been removed
              clearInterval(checkInterval);
              clearInterval(flushInterval);
              unsubscribe();
              resolve();
            }
          }, 500);

          // Handle Ctrl+C: Stop cluster since foreground mode has no daemon
          // CRITICAL: In foreground mode, the cluster runs IN this process.
          // If we exit without stopping, the cluster becomes a zombie (state=running but no process).
          process.on('SIGINT', async () => {
            console.log(chalk.dim('\n\n--- Interrupted ---'));
            clearInterval(checkInterval);
            clearInterval(flushInterval);
            unsubscribe();

            // Stop the cluster properly so state is updated
            try {
              console.log(chalk.dim(`Stopping cluster ${id}...`));
              await orchestrator.stop(id);
              console.log(chalk.dim(`Cluster ${id} stopped.`));
            } catch (stopErr) {
              console.error(chalk.red(`Failed to stop cluster: ${stopErr.message}`));
            }

            process.exit(0);
          });
        });

        console.log(chalk.dim(`\nCluster ${id} completed.`));
      } else {
        // Try resuming as task
        const { resumeTask } = await import('../task-lib/commands/resume.js');
        await resumeTask(id, prompt);
      }
    } catch (error) {
      console.error(chalk.red('Error resuming:'), error.message);
      process.exit(1);
    }
  });

// Finish cluster - convert to single-agent completion task
program
  .command('finish <id>')
  .description('Take existing cluster and create completion-focused task (creates PR and merges)')
  .option('-y, --yes', 'Skip confirmation if cluster is running')
  .action(async (id, options) => {
    try {
      const OrchestratorModule = require('../src/orchestrator');
      const orchestrator = new OrchestratorModule();

      // Check if cluster exists
      const cluster = orchestrator.getCluster(id);

      if (!cluster) {
        console.error(chalk.red(`Error: Cluster ${id} not found`));
        console.error(chalk.dim('Use "zeroshot list" to see available clusters'));
        process.exit(1);
      }

      // Stop cluster if it's running (with confirmation unless -y)
      if (cluster.state === 'running') {
        if (!options.y && !options.yes) {
          console.log(chalk.yellow(`Cluster ${id} is still running.`));
          console.log(chalk.dim('Stopping it before converting to completion task...'));
          console.log('');

          // Simple confirmation prompt
          const readline = require('readline');
          const rl = readline.createInterface({
            input: process.stdin,
            output: process.stdout,
          });

          const answer = await new Promise((resolve) => {
            rl.question(chalk.yellow('Continue? (y/N) '), resolve);
          });
          rl.close();

          if (answer.toLowerCase() !== 'y' && answer.toLowerCase() !== 'yes') {
            console.log(chalk.red('Aborted'));
            process.exit(0);
          }
        }

        console.log(chalk.cyan('Stopping cluster...'));
        await orchestrator.stop(id);
        console.log(chalk.green('✓ Cluster stopped'));
        console.log('');
      }

      console.log(chalk.cyan(`Converting cluster ${id} to completion task...`));
      console.log('');

      // Extract cluster context from ledger
      const messages = cluster.messageBus.getAll(id);

      // Find original task
      const issueOpened = messages.find((m) => m.topic === 'ISSUE_OPENED');
      const taskText = issueOpened?.content?.text || 'Unknown task';
      const issueNumber = issueOpened?.content?.data?.issue_number;
      const issueTitle = issueOpened?.content?.data?.title || 'Implementation';

      // Find what's been done
      const agentOutputs = messages.filter((m) => m.topic === 'AGENT_OUTPUT');
      const validations = messages.filter((m) => m.topic === 'VALIDATION_RESULT');

      // Build context summary
      let contextSummary = `# Original Task\n\n${taskText}\n\n`;

      if (issueNumber) {
        contextSummary += `Issue: #${issueNumber} - ${issueTitle}\n\n`;
      }

      contextSummary += `# Progress So Far\n\n`;
      contextSummary += `- ${agentOutputs.length} agent outputs\n`;
      contextSummary += `- ${validations.length} validation results\n`;

      const approvedValidations = validations.filter(
        (v) => v.content?.data?.approved === true || v.content?.data?.approved === 'true'
      );
      contextSummary += `- ${approvedValidations.length} approvals\n\n`;

      // Add recent validation summaries
      if (validations.length > 0) {
        contextSummary += `## Recent Validations\n\n`;
        for (const v of validations.slice(-3)) {
          const approved =
            v.content?.data?.approved === true || v.content?.data?.approved === 'true';
          const icon = approved ? '✅' : '❌';
          contextSummary += `${icon} **${v.sender}**: ${v.content?.data?.summary || 'No summary'}\n`;
        }
        contextSummary += '\n';
      }

      // Build ultra-aggressive completion prompt (always merges)
      const mergeGoal = 'CREATE PR AND MERGE IT';
      const mergeStep = `
8. MERGE THE PR - THIS IS MANDATORY:
   \`\`\`bash
   gh pr merge --merge --auto
   \`\`\`

   IF MERGE FAILS DUE TO CONFLICTS:
   a) Pull and rebase: \`git fetch origin main && git rebase origin/main\`
   b) Resolve conflicts manually (read files, make decisions, edit)
   c) \`git add <resolved-files> && git rebase --continue\`
   d) Force push: \`git push --force-with-lease\`
   e) Retry merge: \`gh pr merge --merge\`

   REPEAT UNTIL MERGED. DO NOT GIVE UP.`;

      const completionPrompt = `# YOUR MISSION: ${mergeGoal}

${contextSummary}

# WHAT YOU MUST DO NOW

You are the FINISHER. Your ONLY job is to take this cluster's work and push it across the finish line.

**FAILURE IS NOT AN OPTION. YOU MUST COMPLETE THIS TASK.**

## Steps (MUST complete ALL):

1. REVIEW CURRENT STATE - Check git status, see what's changed:
   \`\`\`bash
   git status
   git diff
   \`\`\`

2. COMMIT ALL CHANGES - Stage and commit everything:
   \`\`\`bash
   git add .
   git commit -m "${issueTitle || 'feat: implement task'}"
   \`\`\`

3. CREATE BRANCH - Use issue number if available:
   \`\`\`bash
   ${issueNumber ? `git checkout -b issue-${issueNumber}` : 'git checkout -b feature/implementation'}
   \`\`\`

4. PUSH TO REMOTE:
   \`\`\`bash
   git push -u origin HEAD
   \`\`\`

5. CREATE PULL REQUEST:
   \`\`\`bash
   gh pr create --title "${issueTitle || 'Implementation'}" --body "Closes #${issueNumber || 'N/A'}

## Summary
${taskText.slice(0, 200)}...

## Changes
- Implementation complete
- All validations addressed

🤖 Generated with zeroshot finish"
   \`\`\`

6. GET PR URL:
   \`\`\`bash
   gh pr view --json url -q .url
   \`\`\`

7. OUTPUT THE PR URL - Print it clearly so user can see it
${mergeStep}

## RULES

- NO EXCUSES: If something fails, FIX IT and retry
- NO SHORTCUTS: Follow ALL steps above
- NO PARTIAL WORK: Must reach PR creation and merge
- IF TESTS FAIL: Fix them until they pass
- IF CI FAILS: Wait for it, fix issues, retry
- IF CONFLICTS: Resolve them intelligently

**DO NOT STOP UNTIL YOU HAVE A MERGED PR.**`;

      // Show preview
      console.log(chalk.dim('='.repeat(80)));
      console.log(chalk.dim('Task prompt preview:'));
      console.log(chalk.dim('='.repeat(80)));
      console.log(completionPrompt.split('\n').slice(0, 20).join('\n'));
      console.log(chalk.dim('... (truncated) ...\n'));
      console.log(chalk.dim('='.repeat(80)));
      console.log('');

      // Launch as task (preserve isolation if cluster was isolated)
      console.log(chalk.cyan('Launching completion task...'));
      const { runTask } = await import('../task-lib/commands/run.js');

      const taskOptions = {
        cwd: process.cwd(),
      };

      // If cluster was in isolation mode, pass container info to task
      if (cluster.isolation?.enabled && cluster.isolation?.containerId) {
        console.log(chalk.dim(`Using isolation container: ${cluster.isolation.containerId}`));
        taskOptions.isolation = {
          containerId: cluster.isolation.containerId,
          workDir: '/workspace', // Standard workspace mount point in isolation containers
        };
      }

      await runTask(completionPrompt, taskOptions);

      console.log('');
      console.log(chalk.green(`✓ Completion task started`));
      if (cluster.isolation?.enabled) {
        console.log(chalk.dim('Running in isolation container (same as cluster)'));
      }
      console.log(chalk.dim('Monitor with: zeroshot list'));
    } catch (error) {
      console.error(chalk.red('Error:'), error.message);
      process.exit(1);
    }
  });

// Clean tasks
program
  .command('clean')
  .description('Remove old task records and logs')
  .option('-a, --all', 'Remove all tasks')
  .option('-c, --completed', 'Remove completed tasks')
  .option('-f, --failed', 'Remove failed/stale/killed tasks')
  .action(async (options) => {
    try {
      const { cleanTasks } = await import('../task-lib/commands/clean.js');
      await cleanTasks(options);
    } catch (error) {
      console.error('Error cleaning tasks:', error.message);
      process.exit(1);
    }
  });

// Purge all runs (clusters + tasks) - NUCLEAR option
program
  .command('purge')
  .description('NUCLEAR: Kill all running processes and delete all data')
  .option('-y, --yes', 'Skip confirmation')
  .action(async (options) => {
    try {
      const orchestrator = getOrchestrator();

      // Get counts first
      const clusters = orchestrator.listClusters();
      const runningClusters = clusters.filter(
        (c) => c.state === 'running' || c.state === 'initializing'
      );

      const { loadTasks } = await import('../task-lib/store.js');
      const { isProcessRunning } = await import('../task-lib/runner.js');
      const tasks = Object.values(loadTasks());
      const runningTasks = tasks.filter((t) => t.status === 'running' && isProcessRunning(t.pid));

      // Check if there's anything to clear
      if (clusters.length === 0 && tasks.length === 0) {
        console.log(chalk.dim('No clusters or tasks to clear.'));
        return;
      }

      // Show what will be cleared
      console.log(chalk.bold('\nWill kill and delete:'));
      if (clusters.length > 0) {
        console.log(chalk.cyan(`  ${clusters.length} cluster(s) with all history`));
        if (runningClusters.length > 0) {
          console.log(chalk.yellow(`    ${runningClusters.length} running`));
        }
      }
      if (tasks.length > 0) {
        console.log(chalk.yellow(`  ${tasks.length} task(s) with all logs`));
        if (runningTasks.length > 0) {
          console.log(chalk.yellow(`    ${runningTasks.length} running`));
        }
      }
      console.log('');

      // Confirm unless -y flag
      if (!options.yes) {
        const readline = require('readline');
        const rl = readline.createInterface({
          input: process.stdin,
          output: process.stdout,
        });

        const answer = await new Promise((resolve) => {
          rl.question(
            chalk.bold.red(
              'This will kill all processes and permanently delete all data. Proceed? [y/N] '
            ),
            resolve
          );
        });
        rl.close();

        if (answer.toLowerCase() !== 'y') {
          console.log('Aborted.');
          return;
        }
      }

      console.log('');

      // Kill running clusters first
      if (runningClusters.length > 0) {
        console.log(chalk.bold('Killing running clusters...'));
        const clusterResults = await orchestrator.killAll();
        for (const id of clusterResults.killed) {
          console.log(chalk.green(`✓ Killed cluster: ${id}`));
        }
        for (const err of clusterResults.errors) {
          console.log(chalk.red(`✗ Failed to kill cluster ${err.id}: ${err.error}`));
        }
      }

      // Kill running tasks
      if (runningTasks.length > 0) {
        console.log(chalk.bold('Killing running tasks...'));
        const { killTask } = await import('../task-lib/runner.js');
        const { updateTask } = await import('../task-lib/store.js');

        for (const task of runningTasks) {
          if (!isProcessRunning(task.pid)) {
            updateTask(task.id, {
              status: 'stale',
              error: 'Process died unexpectedly',
            });
            console.log(chalk.yellow(`○ Task ${task.id} was already dead, marked stale`));
            continue;
          }

          const killed = killTask(task.pid);
          if (killed) {
            updateTask(task.id, { status: 'killed', error: 'Killed by clear' });
            console.log(chalk.green(`✓ Killed task: ${task.id}`));
          } else {
            console.log(chalk.red(`✗ Failed to kill task: ${task.id}`));
          }
        }
      }

      // Delete all cluster data
      if (clusters.length > 0) {
        console.log(chalk.bold('Deleting cluster data...'));
        const clustersFile = path.join(orchestrator.storageDir, 'clusters.json');
        const clustersDir = path.join(orchestrator.storageDir, 'clusters');

        // Delete all cluster databases
        for (const cluster of clusters) {
          const dbPath = path.join(orchestrator.storageDir, `${cluster.id}.db`);
          if (fs.existsSync(dbPath)) {
            fs.unlinkSync(dbPath);
            console.log(chalk.green(`✓ Deleted cluster database: ${cluster.id}.db`));
          }
        }

        // Delete clusters.json
        if (fs.existsSync(clustersFile)) {
          fs.unlinkSync(clustersFile);
          console.log(chalk.green(`✓ Deleted clusters.json`));
        }

        // Delete clusters directory if exists
        if (fs.existsSync(clustersDir)) {
          fs.rmSync(clustersDir, { recursive: true, force: true });
          console.log(chalk.green(`✓ Deleted clusters/ directory`));
        }

        // Clear in-memory clusters
        orchestrator.clusters.clear();
      }

      // Delete all task data
      if (tasks.length > 0) {
        console.log(chalk.bold('Deleting task data...'));
        const { cleanTasks } = await import('../task-lib/commands/clean.js');
        await cleanTasks({ all: true });
      }

      console.log(chalk.bold.green('\nAll runs purged.'));
    } catch (error) {
      console.error('Error clearing runs:', error.message);
      process.exit(1);
    }
  });

// Schedule a task
program
  .command('schedule <prompt>')
  .description('Create a recurring scheduled task')
  .option('-e, --every <interval>', 'Interval (e.g., "1h", "30m", "1d")')
  .option('--cron <expression>', 'Cron expression')
  .option('-C, --cwd <path>', 'Working directory')
  .action(async (prompt, options) => {
    try {
      const { createSchedule } = await import('../task-lib/commands/schedule.js');
      await createSchedule(prompt, options);
    } catch (error) {
      console.error('Error creating schedule:', error.message);
      process.exit(1);
    }
  });

// List schedules
program
  .command('schedules')
  .description('List all scheduled tasks')
  .action(async () => {
    try {
      const { listSchedules } = await import('../task-lib/commands/schedules.js');
      await listSchedules();
    } catch (error) {
      console.error('Error listing schedules:', error.message);
      process.exit(1);
    }
  });

// Unschedule a task
program
  .command('unschedule <scheduleId>')
  .description('Remove a scheduled task')
  .action(async (scheduleId) => {
    try {
      const { deleteSchedule } = await import('../task-lib/commands/unschedule.js');
      await deleteSchedule(scheduleId);
    } catch (error) {
      console.error('Error unscheduling:', error.message);
      process.exit(1);
    }
  });

// Scheduler daemon management
program
  .command('scheduler <action>')
  .description('Manage scheduler daemon (start, stop, status, logs)')
  .action(async (action) => {
    try {
      const { schedulerCommand } = await import('../task-lib/commands/scheduler-cmd.js');
      await schedulerCommand(action);
    } catch (error) {
      console.error('Error managing scheduler:', error.message);
      process.exit(1);
    }
  });

// Get log path (machine-readable)
program
  .command('get-log-path <taskId>')
  .description('Output log file path for a task (machine-readable)')
  .action(async (taskId) => {
    try {
      const { getLogPath } = await import('../task-lib/commands/get-log-path.js');
      await getLogPath(taskId);
    } catch (error) {
      console.error('Error getting log path:', error.message);
      process.exit(1);
    }
  });

// Watch command - interactive TUI dashboard
program
  .command('watch')
  .description('Interactive TUI to monitor clusters')
  .option('--refresh-rate <ms>', 'Refresh interval in milliseconds', '1000')
  .action(async (options) => {
    try {
      const TUI = require('../src/tui');
      const tui = new TUI({
        orchestrator: getOrchestrator(),
        refreshRate: parseInt(options.refreshRate, 10),
      });
      await tui.start();
    } catch (error) {
      console.error('Error starting TUI:', error.message);
      process.exit(1);
    }
  });

// Settings management
const settingsCmd = program.command('settings').description('Manage zeroshot settings');

settingsCmd
  .command('list')
  .description('Show all settings')
  .action(() => {
    const settings = loadSettings();
    console.log(chalk.bold('\nCrew Settings:\n'));
    for (const [key, value] of Object.entries(settings)) {
      const isDefault = DEFAULT_SETTINGS[key] === value;
      const label = isDefault ? chalk.dim(key) : chalk.cyan(key);
      const val = isDefault ? chalk.dim(String(value)) : chalk.white(String(value));
      console.log(`  ${label.padEnd(30)} ${val}`);
    }
    console.log('');
  });

settingsCmd
  .command('get <key>')
  .description('Get a setting value')
  .action((key) => {
    const settings = loadSettings();
    if (!(key in settings)) {
      console.error(chalk.red(`Unknown setting: ${key}`));
      console.log(chalk.dim('\nAvailable settings:'));
      Object.keys(DEFAULT_SETTINGS).forEach((k) => console.log(chalk.dim(`  - ${k}`)));
      process.exit(1);
    }
    console.log(settings[key]);
  });

settingsCmd
  .command('set <key> <value>')
  .description('Set a setting value')
  .action((key, value) => {
    if (!(key in DEFAULT_SETTINGS)) {
      console.error(chalk.red(`Unknown setting: ${key}`));
      console.log(chalk.dim('\nAvailable settings:'));
      Object.keys(DEFAULT_SETTINGS).forEach((k) => console.log(chalk.dim(`  - ${k}`)));
      process.exit(1);
    }

    const settings = loadSettings();

    // Type coercion
    let parsedValue;
    try {
      parsedValue = coerceValue(key, value);
    } catch (error) {
      console.error(chalk.red(error.message));
      process.exit(1);
    }

    // Validation
    const validationError = validateSetting(key, parsedValue);
    if (validationError) {
      console.error(chalk.red(validationError));
      process.exit(1);
    }

    settings[key] = parsedValue;
    saveSettings(settings);
    console.log(chalk.green(`✓ Set ${key} = ${parsedValue}`));
  });

settingsCmd
  .command('reset')
  .description('Reset all settings to defaults')
  .option('-y, --yes', 'Skip confirmation')
  .action((options) => {
    if (!options.yes) {
      const readline = require('readline');
      const rl = readline.createInterface({
        input: process.stdin,
        output: process.stdout,
      });

      rl.question(chalk.yellow('Reset all settings to defaults? [y/N] '), (answer) => {
        rl.close();
        if (answer.toLowerCase() !== 'y') {
          console.log('Aborted.');
          return;
        }
        saveSettings(DEFAULT_SETTINGS);
        console.log(chalk.green('✓ Settings reset to defaults'));
      });
    } else {
      saveSettings(DEFAULT_SETTINGS);
      console.log(chalk.green('✓ Settings reset to defaults'));
    }
  });

// Add alias for settings list (just `zeroshot settings`)
settingsCmd.action(() => {
  // Default action when no subcommand - show list
  const settings = loadSettings();
  console.log(chalk.bold('\nCrew Settings:\n'));
  for (const [key, value] of Object.entries(settings)) {
    const isDefault = DEFAULT_SETTINGS[key] === value;
    const label = isDefault ? chalk.dim(key) : chalk.cyan(key);
    const val = isDefault ? chalk.dim(String(value)) : chalk.white(String(value));
    console.log(`  ${label.padEnd(30)} ${val}`);
  }
  console.log('');
  console.log(chalk.dim('Usage:'));
  console.log(chalk.dim('  zeroshot settings set <key> <value>'));
  console.log(chalk.dim('  zeroshot settings get <key>'));
  console.log(chalk.dim('  zeroshot settings reset'));
  console.log('');
});

// Config visualization commands
const configCmd = program.command('config').description('Manage and visualize cluster configs');

configCmd
  .command('list')
  .description('List available cluster configs')
  .action(() => {
    try {
      const configsDir = path.join(PACKAGE_ROOT, 'cluster-templates');
      const files = fs.readdirSync(configsDir).filter((f) => f.endsWith('.json'));

      if (files.length === 0) {
        console.log(chalk.dim('No configs found in examples/'));
        return;
      }

      console.log(chalk.bold('\nAvailable configs:\n'));
      for (const file of files) {
        const configPath = path.join(configsDir, file);
        const config = JSON.parse(fs.readFileSync(configPath, 'utf8'));
        const agentCount = config.agents?.length || 0;
        const name = file.replace('.json', '');

        console.log(`  ${chalk.cyan(name.padEnd(30))} ${chalk.dim(`${agentCount} agents`)}`);
      }
      console.log('');
    } catch (error) {
      console.error('Error listing configs:', error.message);
      process.exit(1);
    }
  });

configCmd
  .command('show <name>')
  .description('Visualize a cluster config')
  .action((name) => {
    try {
      // Support both with and without .json extension
      const configName = name.endsWith('.json') ? name : `${name}.json`;
      const configPath = path.join(PACKAGE_ROOT, 'cluster-templates', configName);

      if (!fs.existsSync(configPath)) {
        console.error(chalk.red(`Config not found: ${configName}`));
        console.log(chalk.dim('\nAvailable configs:'));
        const files = fs
          .readdirSync(path.join(PACKAGE_ROOT, 'cluster-templates'))
          .filter((f) => f.endsWith('.json'));
        files.forEach((f) => console.log(chalk.dim(`  - ${f.replace('.json', '')}`)));
        process.exit(1);
      }

      const config = JSON.parse(fs.readFileSync(configPath, 'utf8'));

      // Header
      console.log('');
      console.log(chalk.bold.cyan('═'.repeat(80)));
      console.log(chalk.bold.cyan(`  Config: ${name.replace('.json', '')}`));
      console.log(chalk.bold.cyan('═'.repeat(80)));
      console.log('');

      // Agents section
      console.log(chalk.bold('Agents:\n'));

      if (!config.agents || config.agents.length === 0) {
        console.log(chalk.dim('  No agents defined'));
      } else {
        for (const agent of config.agents) {
          const color = getColorForSender(agent.id);
          console.log(color.bold(`  ${agent.id}`));
          console.log(chalk.dim(`    Role: ${agent.role || 'none'}`));

          if (agent.model) {
            console.log(chalk.dim(`    Model: ${agent.model}`));
          }

          if (agent.triggers && agent.triggers.length > 0) {
            // Triggers are objects with topic field
            const triggerTopics = agent.triggers
              .map((t) => (typeof t === 'string' ? t : t.topic))
              .filter(Boolean);
            console.log(chalk.dim(`    Triggers: ${triggerTopics.join(', ')}`));
          } else {
            console.log(chalk.dim(`    Triggers: none (manual only)`));
          }

          console.log('');
        }
      }

      // Message flow visualization
      if (config.agents && config.agents.length > 0) {
        console.log(chalk.bold('Message Flow:\n'));

        // Build trigger map: topic -> [agents that listen]
        const triggerMap = new Map();
        for (const agent of config.agents) {
          if (agent.triggers) {
            for (const trigger of agent.triggers) {
              const topic = typeof trigger === 'string' ? trigger : trigger.topic;
              if (topic) {
                if (!triggerMap.has(topic)) {
                  triggerMap.set(topic, []);
                }
                triggerMap.get(topic).push(agent.id);
              }
            }
          }
        }

        if (triggerMap.size === 0) {
          console.log(chalk.dim('  No automatic triggers defined\n'));
        } else {
          for (const [topic, agents] of triggerMap) {
            console.log(
              `  ${chalk.yellow(topic)} ${chalk.dim('→')} ${agents.map((a) => getColorForSender(a)(a)).join(', ')}`
            );
          }
          console.log('');
        }
      }

      console.log(chalk.bold.cyan('═'.repeat(80)));
      console.log('');
    } catch (error) {
      console.error('Error showing config:', error.message);
      process.exit(1);
    }
  });

configCmd
  .command('validate <configPath>')
  .description('Validate a cluster config for structural issues')
  .option('--strict', 'Treat warnings as errors')
  .option('--json', 'Output as JSON')
  .action((configPath, options) => {
    try {
      const { validateConfig, formatValidationResult } = require('../src/config-validator');

      // Resolve path (support relative paths and built-in names)
      let fullPath;
      if (fs.existsSync(configPath)) {
        fullPath = path.resolve(configPath);
      } else {
        // Try examples directory
        const configName = configPath.endsWith('.json') ? configPath : `${configPath}.json`;
        fullPath = path.join(PACKAGE_ROOT, 'cluster-templates', configName);
        if (!fs.existsSync(fullPath)) {
          console.error(chalk.red(`Config not found: ${configPath}`));
          console.log(chalk.dim('\nAvailable built-in configs:'));
          const files = fs
            .readdirSync(path.join(PACKAGE_ROOT, 'cluster-templates'))
            .filter((f) => f.endsWith('.json'));
          files.forEach((f) => console.log(chalk.dim(`  - ${f.replace('.json', '')}`)));
          process.exit(1);
        }
      }

      // Load and validate
      const config = JSON.parse(fs.readFileSync(fullPath, 'utf8'));
      const result = validateConfig(config);

      // Apply strict mode
      if (options.strict && result.warnings.length > 0) {
        result.errors.push(...result.warnings.map((w) => `[strict] ${w}`));
        result.valid = false;
      }

      // Output
      if (options.json) {
        console.log(JSON.stringify(result, null, 2));
      } else {
        console.log('');
        console.log(chalk.bold(`Validating: ${path.basename(fullPath)}`));
        console.log('');
        console.log(formatValidationResult(result));
        console.log('');
      }

      // Exit code
      process.exit(result.valid ? 0 : 1);
    } catch (error) {
      if (error instanceof SyntaxError) {
        console.error(chalk.red(`Invalid JSON: ${error.message}`));
      } else {
        console.error(chalk.red(`Error: ${error.message}`));
      }
      process.exit(1);
    }
  });

// Agent library commands
const agentsCmd = program.command('agents').description('View available agent definitions');

agentsCmd
  .command('list')
  .alias('ls')
  .description('List available agent definitions')
  .option('--verbose', 'Show full agent details')
  .option('--json', 'Output as JSON')
  .action((options) => {
    try {
      const agentsDir = path.join(PACKAGE_ROOT, 'src', 'agents');

      // Check if agents directory exists
      if (!fs.existsSync(agentsDir)) {
        if (options.json) {
          console.log(JSON.stringify({ agents: [], error: null }, null, 2));
        } else {
          console.log(chalk.dim('No agents directory found.'));
        }
        return;
      }

      const files = fs.readdirSync(agentsDir).filter((f) => f.endsWith('.json'));

      if (files.length === 0) {
        if (options.json) {
          console.log(JSON.stringify({ agents: [], error: null }, null, 2));
        } else {
          console.log(chalk.dim('No agent definitions found in src/agents/'));
        }
        return;
      }

      // Parse all agent files
      const agents = [];
      for (const file of files) {
        try {
          const agentPath = path.join(agentsDir, file);
          const agent = JSON.parse(fs.readFileSync(agentPath, 'utf8'));
          agents.push({
            file: file.replace('.json', ''),
            id: agent.id || file.replace('.json', ''),
            role: agent.role || 'unspecified',
            model: agent.model || 'default',
            triggers: agent.triggers?.length || 0,
            prompt: agent.prompt || null,
            output: agent.output || null,
          });
        } catch (err) {
          // Skip invalid JSON files
          console.error(chalk.yellow(`Warning: Could not parse ${file}: ${err.message}`));
        }
      }

      // JSON output
      if (options.json) {
        console.log(JSON.stringify({ agents, error: null }, null, 2));
        return;
      }

      // Human-readable output
      console.log(chalk.bold('\nAvailable agent definitions:\n'));

      for (const agent of agents) {
        console.log(
          `  ${chalk.cyan(agent.id.padEnd(25))} ${chalk.dim('role:')} ${agent.role.padEnd(20)} ${chalk.dim('model:')} ${agent.model}`
        );

        if (options.verbose) {
          console.log(chalk.dim(`    Triggers: ${agent.triggers}`));
          if (agent.output) {
            console.log(chalk.dim(`    Output topic: ${agent.output.topic || 'none'}`));
          }
          if (agent.prompt) {
            const promptPreview = agent.prompt.substring(0, 100).replace(/\n/g, ' ');
            console.log(chalk.dim(`    Prompt: ${promptPreview}...`));
          }
          console.log('');
        }
      }

      if (!options.verbose) {
        console.log('');
        console.log(chalk.dim('  Use --verbose for full details'));
      }
      console.log('');
    } catch (error) {
      if (options.json) {
        console.log(JSON.stringify({ agents: [], error: error.message }, null, 2));
      } else {
        console.error(chalk.red(`Error listing agents: ${error.message}`));
      }
      process.exit(1);
    }
  });

agentsCmd
  .command('show <name>')
  .description('Show detailed agent definition')
  .option('--json', 'Output as JSON')
  .action((name, options) => {
    try {
      const agentsDir = path.join(PACKAGE_ROOT, 'src', 'agents');

      // Support both with and without .json extension
      const agentName = name.endsWith('.json') ? name : `${name}.json`;
      const agentPath = path.join(agentsDir, agentName);

      if (!fs.existsSync(agentPath)) {
        // Try with -agent.json suffix
        const altPath = path.join(agentsDir, `${name}-agent.json`);
        if (fs.existsSync(altPath)) {
          const agent = JSON.parse(fs.readFileSync(altPath, 'utf8'));
          outputAgent(agent, options);
          return;
        }

        if (options.json) {
          console.log(JSON.stringify({ error: `Agent not found: ${name}` }, null, 2));
        } else {
          console.error(chalk.red(`Agent not found: ${name}`));
          console.log(chalk.dim('\nAvailable agents:'));
          const files = fs.readdirSync(agentsDir).filter((f) => f.endsWith('.json'));
          files.forEach((f) => console.log(chalk.dim(`  - ${f.replace('.json', '')}`)));
        }
        process.exit(1);
      }

      const agent = JSON.parse(fs.readFileSync(agentPath, 'utf8'));
      outputAgent(agent, options);
    } catch (error) {
      if (options.json) {
        console.log(JSON.stringify({ error: error.message }, null, 2));
      } else {
        console.error(chalk.red(`Error: ${error.message}`));
      }
      process.exit(1);
    }
  });

function outputAgent(agent, options) {
  if (options.json) {
    console.log(JSON.stringify(agent, null, 2));
    return;
  }

  // Human-readable output
  console.log('');
  console.log(chalk.bold.cyan('═'.repeat(80)));
  console.log(chalk.bold.cyan(`  Agent: ${agent.id}`));
  console.log(chalk.bold.cyan('═'.repeat(80)));
  console.log('');

  // Basic info
  console.log(chalk.bold('Configuration:'));
  console.log(`  ${chalk.dim('ID:')}    ${agent.id}`);
  console.log(`  ${chalk.dim('Role:')}  ${agent.role || 'unspecified'}`);
  console.log(`  ${chalk.dim('Model:')} ${agent.model || 'default'}`);
  console.log('');

  // Triggers
  if (agent.triggers && agent.triggers.length > 0) {
    console.log(chalk.bold('Triggers:'));
    for (const trigger of agent.triggers) {
      console.log(`  ${chalk.yellow('•')} Topic: ${chalk.cyan(trigger.topic)}`);
      if (trigger.action) {
        console.log(`    Action: ${trigger.action}`);
      }
      if (trigger.logic?.script) {
        const scriptPreview = trigger.logic.script.substring(0, 80).replace(/\n/g, ' ');
        console.log(chalk.dim(`    Logic: ${scriptPreview}...`));
      }
    }
    console.log('');
  }

  // Output
  if (agent.output) {
    console.log(chalk.bold('Output:'));
    console.log(`  ${chalk.dim('Topic:')} ${agent.output.topic || 'none'}`);
    if (agent.output.publishAfter) {
      console.log(`  ${chalk.dim('Publish after:')} ${agent.output.publishAfter}`);
    }
    console.log('');
  }

  // Prompt
  if (agent.prompt) {
    console.log(chalk.bold('Prompt:'));
    console.log(chalk.dim('─'.repeat(76)));
    // Show first 500 chars of prompt
    const promptLines = agent.prompt.substring(0, 500).split('\n');
    for (const line of promptLines) {
      console.log(`  ${line}`);
    }
    if (agent.prompt.length > 500) {
      console.log(chalk.dim(`  ... (${agent.prompt.length - 500} more characters)`));
    }
    console.log(chalk.dim('─'.repeat(76)));
    console.log('');
  }
}

// Helper function to keep the process alive for follow mode
function keepProcessAlive(cleanupFn) {
  // Prevent Node.js from exiting by keeping the event loop active
  // Use setInterval with a long interval (1 hour) to minimize overhead
  const keepAliveInterval = setInterval(() => {}, 3600000);

  // Handle graceful shutdown on Ctrl+C
  process.on('SIGINT', () => {
    clearInterval(keepAliveInterval);
    if (cleanupFn) cleanupFn();
    console.log('\n\nStopped following logs.');
    process.exit(0);
  });

  // Also handle SIGTERM for graceful shutdown
  process.on('SIGTERM', () => {
    clearInterval(keepAliveInterval);
    if (cleanupFn) cleanupFn();
    process.exit(0);
  });
}

// Tool icons for different tool types
function getToolIcon(toolName) {
  const icons = {
    Read: '📖',
    Write: '📝',
    Edit: '✏️',
    Bash: '💻',
    Glob: '🔍',
    Grep: '🔎',
    WebFetch: '🌐',
    WebSearch: '🔎',
    Task: '🤖',
    TodoWrite: '📋',
    AskUserQuestion: '❓',
  };
  return icons[toolName] || '🔧';
}

// Format tool call input for display
function formatToolCall(toolName, input) {
  if (!input) return '';

  switch (toolName) {
    case 'Bash':
      return input.command ? `$ ${input.command}` : '';
    case 'Read':
      return input.file_path ? input.file_path.split('/').slice(-2).join('/') : '';
    case 'Write':
      return input.file_path ? `→ ${input.file_path.split('/').slice(-2).join('/')}` : '';
    case 'Edit':
      return input.file_path ? input.file_path.split('/').slice(-2).join('/') : '';
    case 'Glob':
      return input.pattern || '';
    case 'Grep':
      return input.pattern ? `/${input.pattern}/` : '';
    case 'WebFetch':
      return input.url ? input.url.substring(0, 50) : '';
    case 'WebSearch':
      return input.query ? `"${input.query}"` : '';
    case 'Task':
      return input.description || '';
    case 'TodoWrite':
      if (input.todos && Array.isArray(input.todos)) {
        const statusCounts = {};
        input.todos.forEach((todo) => {
          statusCounts[todo.status] = (statusCounts[todo.status] || 0) + 1;
        });
        const parts = Object.entries(statusCounts).map(
          ([status, count]) => `${count} ${status.replace('_', ' ')}`
        );
        return `${input.todos.length} todo${input.todos.length === 1 ? '' : 's'} (${parts.join(', ')})`;
      }
      return '';
    case 'AskUserQuestion':
      if (input.questions && Array.isArray(input.questions)) {
        const q = input.questions[0];
        const preview = q.question.substring(0, 50);
        return input.questions.length > 1
          ? `${input.questions.length} questions: "${preview}..."`
          : `"${preview}${q.question.length > 50 ? '...' : ''}"`;
      }
      return '';
    default:
      // For unknown tools, show first key-value pair
      const keys = Object.keys(input);
      if (keys.length > 0) {
        const val = String(input[keys[0]]).substring(0, 40);
        return val.length < String(input[keys[0]]).length ? val + '...' : val;
      }
      return '';
  }
}

// Format tool result for display
function formatToolResult(content, isError, toolName, toolInput) {
  if (!content) return isError ? 'error' : 'done';

  // For errors, show full message
  if (isError) {
    const firstLine = content.split('\n')[0].substring(0, 80);
    return chalk.red(firstLine);
  }

  // For TodoWrite, show the actual todo items
  if (toolName === 'TodoWrite' && toolInput?.todos && Array.isArray(toolInput.todos)) {
    const todos = toolInput.todos;
    if (todos.length === 0) return chalk.dim('no todos');
    if (todos.length === 1) {
      const status =
        todos[0].status === 'completed' ? '✓' : todos[0].status === 'in_progress' ? '⧗' : '○';
      return chalk.dim(
        `${status} ${todos[0].content.substring(0, 50)}${todos[0].content.length > 50 ? '...' : ''}`
      );
    }
    // Multiple todos - show first one as preview
    const status =
      todos[0].status === 'completed' ? '✓' : todos[0].status === 'in_progress' ? '⧗' : '○';
    return chalk.dim(
      `${status} ${todos[0].content.substring(0, 40)}... (+${todos.length - 1} more)`
    );
  }

  // For success, show summary
  const lines = content.split('\n').filter((l) => l.trim());
  if (lines.length === 0) return 'done';
  if (lines.length === 1) {
    const line = lines[0].substring(0, 60);
    return chalk.dim(line.length < lines[0].length ? line + '...' : line);
  }
  // Multiple lines - show count
  return chalk.dim(`${lines.length} lines`);
}

// Helper function to get deterministic color for an agent/sender based on name hash
// Uses djb2 hash algorithm for good distribution across color palette
// Track recently seen content to avoid duplicates
const recentContentHashes = new Set();
const MAX_RECENT_HASHES = 100;

// Track clusters that have already shown their NEW TASK header (suppress conductor re-publish)
const shownNewTaskForCluster = new Set();

function hashContent(content) {
  // Simple hash for deduplication
  return content.substring(0, 200);
}

function isDuplicate(content) {
  const hash = hashContent(content);
  if (recentContentHashes.has(hash)) {
    return true;
  }
  recentContentHashes.add(hash);
  // Prune old hashes
  if (recentContentHashes.size > MAX_RECENT_HASHES) {
    const arr = Array.from(recentContentHashes);
    recentContentHashes.clear();
    arr.slice(-50).forEach((h) => recentContentHashes.add(h));
  }
  return false;
}

// Format task summary from ISSUE_OPENED message - truncated for display
function formatTaskSummary(issueOpened, maxLen = 35) {
  const data = issueOpened.content?.data || {};
  const issueNum = data.issue_number || data.number;
  const title = data.title;
  const url = data.url || data.html_url;

  // Prefer: #N: Short title
  if (issueNum && title) {
    const truncatedTitle = title.length > maxLen ? title.slice(0, maxLen - 3) + '...' : title;
    return `#${issueNum}: ${truncatedTitle}`;
  }
  if (issueNum) return `#${issueNum}`;

  // Extract from URL
  if (url) {
    const match = url.match(/issues\/(\d+)/);
    if (match) return `#${match[1]}`;
  }

  // Fallback: first meaningful line (for manual prompts)
  const text = issueOpened.content?.text || 'Task';
  const firstLine = text.split('\n').find((l) => l.trim() && !l.startsWith('#')) || 'Task';
  return firstLine.slice(0, maxLen) + (firstLine.length > maxLen ? '...' : '');
}

// Format token usage for display
function formatTokenUsage(tokensByRole) {
  if (!tokensByRole || !tokensByRole._total || tokensByRole._total.count === 0) {
    return null;
  }

  const total = tokensByRole._total;
  const lines = [];

  // Format numbers with commas
  const fmt = (n) => n.toLocaleString();

  // Total line
  const inputTokens = total.inputTokens || 0;
  const outputTokens = total.outputTokens || 0;
  const totalTokens = inputTokens + outputTokens;
  const cost = total.totalCostUsd || 0;

  lines.push(
    chalk.dim('Tokens: ') +
      chalk.cyan(fmt(totalTokens)) +
      chalk.dim(' (') +
      chalk.green(fmt(inputTokens)) +
      chalk.dim(' in / ') +
      chalk.yellow(fmt(outputTokens)) +
      chalk.dim(' out)')
  );

  // Cost line (if available)
  if (cost > 0) {
    lines.push(chalk.dim('Cost: ') + chalk.green('$' + cost.toFixed(4)));
  }

  // Per-role breakdown (compact)
  const roles = Object.keys(tokensByRole).filter((r) => r !== '_total');
  if (roles.length > 1) {
    const roleStats = roles
      .map((role) => {
        const r = tokensByRole[role];
        const roleTotal = (r.inputTokens || 0) + (r.outputTokens || 0);
        return `${role}: ${fmt(roleTotal)}`;
      })
      .join(chalk.dim(' | '));
    lines.push(chalk.dim('By role: ') + roleStats);
  }

  return lines;
}

// Set terminal title (works in most terminals)
function setTerminalTitle(title) {
  // ESC ] 0 ; <title> BEL
  process.stdout.write(`\\x1b]0;${title}\x07`);
}

// Restore terminal title on exit
function restoreTerminalTitle() {
  // Reset to default (empty title lets terminal use its default)
  process.stdout.write('\\x1b]0;\x07');
}

// Format markdown-style text for terminal display
function formatMarkdownLine(line) {
  let formatted = line;

  // Headers: ## Header -> bold cyan
  if (/^#{1,3}\s/.test(formatted)) {
    formatted = formatted.replace(/^#{1,3}\s*/, '');
    return chalk.bold.cyan(formatted);
  }

  // Blockquotes: > text -> dim italic with bar
  if (/^>\s/.test(formatted)) {
    formatted = formatted.replace(/^>\s*/, '');
    return chalk.dim('│ ') + chalk.italic(formatted);
  }

  // Numbered lists: 1. item -> yellow number
  const numMatch = formatted.match(/^(\d+)\.\s+(.*)$/);
  if (numMatch) {
    return chalk.yellow(numMatch[1] + '.') + ' ' + formatInlineMarkdown(numMatch[2]);
  }

  // Bullet lists: - item or * item -> dim bullet
  const bulletMatch = formatted.match(/^[-*]\s+(.*)$/);
  if (bulletMatch) {
    return chalk.dim('•') + ' ' + formatInlineMarkdown(bulletMatch[1]);
  }

  // Checkboxes: - [ ] or - [x]
  const checkMatch = formatted.match(/^[-*]\s+\[([ x])\]\s+(.*)$/i);
  if (checkMatch) {
    const checked = checkMatch[1].toLowerCase() === 'x';
    const icon = checked ? chalk.green('✓') : chalk.dim('○');
    return icon + ' ' + formatInlineMarkdown(checkMatch[2]);
  }

  return formatInlineMarkdown(formatted);
}

// Format inline markdown: **bold**, `code`
function formatInlineMarkdown(text) {
  let result = text;

  // Bold: **text** -> bold
  result = result.replace(/\*\*([^*]+)\*\*/g, (_, content) => chalk.bold(content));

  // Inline code: `code` -> cyan dim
  result = result.replace(/`([^`]+)`/g, (_, content) => chalk.cyan.dim(content));

  return result;
}

// Line buffer per sender - tracks line state for prefix printing
const lineBuffers = new Map();

// Track current tool call per sender - needed for matching tool results with calls
const currentToolCall = new Map();

/**
 * Render messages to terminal-style output with ANSI colors (same as zeroshot logs)
 */
function renderMessagesToTerminal(clusterId, messages) {
  const lines = [];
  const buffers = new Map(); // Line buffers per sender
  const toolCalls = new Map(); // Track tool calls per sender

  const getBuffer = (sender) => {
    if (!buffers.has(sender)) {
      buffers.set(sender, { text: '', needsPrefix: true });
    }
    return buffers.get(sender);
  };

  const flushBuffer = (sender, prefix) => {
    const buf = buffers.get(sender);
    if (buf && buf.text.trim()) {
      const textLines = buf.text.split('\n');
      for (const line of textLines) {
        if (line.trim()) {
          lines.push(`${prefix} ${formatMarkdownLine(line)}`);
        }
      }
      buf.text = '';
      buf.needsPrefix = true;
    }
  };

  for (const msg of messages) {
    const timestamp = new Date(msg.timestamp).toLocaleTimeString('en-US', {
      hour12: false,
    });
    const color = getColorForSender(msg.sender);
    const prefix = color(`${msg.sender.padEnd(15)} |`);

    // AGENT_LIFECYCLE
    if (msg.topic === 'AGENT_LIFECYCLE') {
      const data = msg.content?.data;
      const event = data?.event;
      let icon, eventText;
      switch (event) {
        case 'STARTED':
          icon = chalk.green('▶');
          const triggers = data.triggers?.join(', ') || 'none';
          eventText = `started (listening for: ${chalk.dim(triggers)})`;
          break;
        case 'TASK_STARTED':
          icon = chalk.yellow('⚡');
          eventText = `${chalk.cyan(data.triggeredBy)} → task #${data.iteration} (${chalk.dim(data.model)})`;
          break;
        case 'TASK_COMPLETED':
          icon = chalk.green('✓');
          eventText = `task #${data.iteration} completed`;
          break;
        default:
          icon = chalk.dim('•');
          eventText = event || 'unknown event';
      }
      lines.push(`${prefix} ${icon} ${eventText}`);
      continue;
    }

    // ISSUE_OPENED
    if (msg.topic === 'ISSUE_OPENED') {
      lines.push('');
      lines.push(chalk.bold.blue('─'.repeat(80)));
      // Extract issue URL if present
      const issueData = msg.content?.data || {};
      const issueUrl = issueData.url || issueData.html_url;
      const issueTitle = issueData.title;
      const issueNum = issueData.issue_number || issueData.number;

      if (issueUrl) {
        lines.push(
          `${prefix} ${chalk.gray(timestamp)} ${chalk.bold.blue('📋')} ${chalk.cyan(issueUrl)}`
        );
        if (issueTitle) {
          lines.push(`${prefix} ${chalk.white(issueTitle)}`);
        }
      } else if (issueNum) {
        lines.push(
          `${prefix} ${chalk.gray(timestamp)} ${chalk.bold.blue('📋')} Issue #${issueNum}`
        );
        if (issueTitle) {
          lines.push(`${prefix} ${chalk.white(issueTitle)}`);
        }
      } else {
        // Fallback: show first line of text only
        lines.push(`${prefix} ${chalk.gray(timestamp)} ${chalk.bold.blue('📋 TASK')}`);
        if (msg.content?.text) {
          const firstLine = msg.content.text
            .split('\n')
            .find((l) => l.trim() && l.trim() !== '# Manual Input');
          if (firstLine) {
            lines.push(`${prefix} ${chalk.white(firstLine.slice(0, 100))}`);
          }
        }
      }
      lines.push(chalk.bold.blue('─'.repeat(80)));
      continue;
    }

    // IMPLEMENTATION_READY
    if (msg.topic === 'IMPLEMENTATION_READY') {
      lines.push(
        `${prefix} ${chalk.gray(timestamp)} ${chalk.bold.yellow('✅ IMPLEMENTATION READY')}`
      );
      continue;
    }

    // VALIDATION_RESULT
    if (msg.topic === 'VALIDATION_RESULT') {
      const data = msg.content?.data || {};
      const approved = data.approved === true || data.approved === 'true';
      const icon = approved ? chalk.green('✓ APPROVED') : chalk.red('✗ REJECTED');
      lines.push(`${prefix} ${chalk.gray(timestamp)} ${chalk.bold.magenta('VALIDATION_RESULT')}`);
      lines.push(`${prefix}   ${icon} ${chalk.dim(data.summary || '')}`);
      if (!approved) {
        let issues = data.issues || data.errors;
        if (typeof issues === 'string') {
          try {
            issues = JSON.parse(issues);
          } catch {
            issues = [];
          }
        }
        if (Array.isArray(issues)) {
          for (const issue of issues) {
            lines.push(`${prefix}     ${chalk.red('•')} ${issue}`);
          }
        }
      }
      continue;
    }

    // PR_CREATED
    if (msg.topic === 'PR_CREATED') {
      lines.push('');
      lines.push(chalk.bold.green('─'.repeat(80)));
      lines.push(`${prefix} ${chalk.gray(timestamp)} ${chalk.bold.green('🔗 PR CREATED')}`);
      if (msg.content?.data?.pr_url) {
        lines.push(`${prefix} ${chalk.cyan(msg.content.data.pr_url)}`);
      }
      if (msg.content?.data?.merged) {
        lines.push(`${prefix} ${chalk.bold.cyan('✓ MERGED')}`);
      }
      lines.push(chalk.bold.green('─'.repeat(80)));
      continue;
    }

    // CLUSTER_COMPLETE
    if (msg.topic === 'CLUSTER_COMPLETE') {
      lines.push('');
      lines.push(chalk.bold.green('─'.repeat(80)));
      lines.push(`${prefix} ${chalk.gray(timestamp)} ${chalk.bold.green('✅ CLUSTER COMPLETE')}`);
      lines.push(chalk.bold.green('─'.repeat(80)));
      continue;
    }

    // AGENT_ERROR
    if (msg.topic === 'AGENT_ERROR') {
      lines.push('');
      lines.push(chalk.bold.red('─'.repeat(80)));
      lines.push(`${prefix} ${chalk.gray(timestamp)} ${chalk.bold.red('🔴 AGENT ERROR')}`);
      if (msg.content?.text) {
        lines.push(`${prefix} ${chalk.red(msg.content.text)}`);
      }
      lines.push(chalk.bold.red('─'.repeat(80)));
      continue;
    }

    // AGENT_OUTPUT - parse streaming JSON
    if (msg.topic === 'AGENT_OUTPUT') {
      const content = msg.content?.data?.line || msg.content?.data?.chunk || msg.content?.text;
      if (!content || !content.trim()) continue;

      const events = parseChunk(content);
      for (const event of events) {
        switch (event.type) {
          case 'text':
            const buf = getBuffer(msg.sender);
            buf.text += event.text;
            // Print complete lines
            while (buf.text.includes('\n')) {
              const idx = buf.text.indexOf('\n');
              const line = buf.text.slice(0, idx);
              buf.text = buf.text.slice(idx + 1);
              if (line.trim()) {
                lines.push(`${prefix} ${formatMarkdownLine(line)}`);
              }
            }
            break;
          case 'tool_call':
            flushBuffer(msg.sender, prefix);
            const icon = getToolIcon(event.toolName);
            const toolDesc = formatToolCall(event.toolName, event.input);
            lines.push(`${prefix} ${icon} ${chalk.cyan(event.toolName)} ${chalk.dim(toolDesc)}`);
            toolCalls.set(msg.sender, {
              toolName: event.toolName,
              input: event.input,
            });
            break;
          case 'tool_result':
            const status = event.isError ? chalk.red('✗') : chalk.green('✓');
            const tc = toolCalls.get(msg.sender);
            const resultDesc = formatToolResult(
              event.content,
              event.isError,
              tc?.toolName,
              tc?.input
            );
            lines.push(`${prefix}   ${status} ${resultDesc}`);
            toolCalls.delete(msg.sender);
            break;
        }
      }
      continue;
    }

    // Other topics - show topic name
    if (msg.topic && !['AGENT_OUTPUT', 'AGENT_LIFECYCLE'].includes(msg.topic)) {
      lines.push(`${prefix} ${chalk.gray(timestamp)} ${chalk.yellow(msg.topic)}`);
    }
  }

  // Flush any remaining buffers
  for (const [sender, buf] of buffers) {
    if (buf.text.trim()) {
      const color = getColorForSender(sender);
      const prefix = color(`${sender.padEnd(15)} |`);
      for (const line of buf.text.split('\n')) {
        if (line.trim()) {
          lines.push(`${prefix} ${line}`);
        }
      }
    }
  }

  return lines.join('\n');
}

// Get terminal width for word wrapping
function getTerminalWidth() {
  return process.stdout.columns || 100;
}

// Word wrap text at terminal width, respecting word boundaries
// Returns array of lines
function wordWrap(text, maxWidth) {
  if (!text || maxWidth <= 0) return [text];

  const words = text.split(/(\s+)/); // Keep whitespace as separate tokens
  const lines = [];
  let currentLine = '';

  for (const word of words) {
    // If adding this word exceeds width, start new line
    if (currentLine.length + word.length > maxWidth && currentLine.trim()) {
      lines.push(currentLine.trimEnd());
      currentLine = word.trimStart(); // Don't start new line with whitespace
    } else {
      currentLine += word;
    }
  }

  if (currentLine.trim()) {
    lines.push(currentLine.trimEnd());
  }

  return lines.length > 0 ? lines : [''];
}

function getLineBuffer(sender) {
  if (!lineBuffers.has(sender)) {
    // needsPrefix: true when at start of new line (need to print prefix)
    // pendingNewline: text written but no newline yet (need newline before next prefix)
    // textBuffer: accumulate text until we have a complete line
    lineBuffers.set(sender, {
      needsPrefix: true,
      pendingNewline: false,
      thinkingNeedsPrefix: true,
      thinkingPendingNewline: false,
      textBuffer: '', // NEW: buffer for accumulating text
    });
  }
  return lineBuffers.get(sender);
}

// Accumulate text and print complete lines only
// Word wrap long lines, aligning continuation with message column
function accumulateText(prefix, sender, text) {
  if (!text) return;
  const buf = getLineBuffer(sender);

  // Add incoming text to buffer
  buf.textBuffer += text;

  // Calculate widths for word wrapping
  const prefixLen = chalk.reset(prefix).replace(/\\x1b\[[0-9;]*m/g, '').length + 1;
  const termWidth = getTerminalWidth();
  const contentWidth = Math.max(40, termWidth - prefixLen - 2);
  const continuationPrefix = ' '.repeat(prefixLen);

  // Process complete lines (ending with \n)
  while (buf.textBuffer.includes('\n')) {
    const newlineIdx = buf.textBuffer.indexOf('\n');
    const completeLine = buf.textBuffer.slice(0, newlineIdx);
    buf.textBuffer = buf.textBuffer.slice(newlineIdx + 1);

    // Word wrap and print the complete line
    // CRITICAL: Batch all output into single safeWrite() to prevent interleaving with render()
    const wrappedLines = wordWrap(completeLine, contentWidth);
    let outputBuffer = '';

    for (let i = 0; i < wrappedLines.length; i++) {
      const wrappedLine = wrappedLines[i];

      // Print prefix (real or continuation)
      if (buf.needsPrefix) {
        outputBuffer += `${prefix} `;
        buf.needsPrefix = false;
      } else if (i > 0) {
        outputBuffer += `${continuationPrefix}`;
      }

      if (wrappedLine.trim()) {
        outputBuffer += formatInlineMarkdown(wrappedLine);
      }

      // Newline after each wrapped segment
      if (i < wrappedLines.length - 1) {
        outputBuffer += '\n';
      }
    }

    // Complete the line
    outputBuffer += '\n';
    buf.needsPrefix = true;
    buf.pendingNewline = false;

    // Single atomic write prevents interleaving
    if (outputBuffer) {
      safeWrite(outputBuffer);
    }
  }

  // Mark that we have pending text (no newline yet)
  if (buf.textBuffer.length > 0) {
    buf.pendingNewline = true;
  }
}

// Stream thinking text immediately with word wrapping
function accumulateThinking(prefix, sender, text) {
  if (!text) return;
  const buf = getLineBuffer(sender);

  // Calculate widths for word wrapping (same as accumulateText but with 💭 prefix)
  const prefixLen = chalk.reset(prefix).replace(/\\x1b\[[0-9;]*m/g, '').length + 4; // +4 for " 💭 "
  const termWidth = getTerminalWidth();
  const contentWidth = Math.max(40, termWidth - prefixLen - 2);
  const continuationPrefix = ' '.repeat(prefixLen);

  let remaining = text;
  while (remaining.length > 0) {
    const newlineIdx = remaining.indexOf('\n');
    const rawLine = newlineIdx === -1 ? remaining : remaining.slice(0, newlineIdx);

    // CRITICAL: Batch all output into single safeWrite() to prevent interleaving with render()
    const wrappedLines = wordWrap(rawLine, contentWidth);
    let outputBuffer = '';

    for (let i = 0; i < wrappedLines.length; i++) {
      const wrappedLine = wrappedLines[i];

      if (buf.thinkingNeedsPrefix) {
        outputBuffer += `${prefix} ${chalk.dim.italic('💭 ')}`;
        buf.thinkingNeedsPrefix = false;
      } else if (i > 0) {
        outputBuffer += `${continuationPrefix}`;
      }

      if (wrappedLine.trim()) {
        outputBuffer += chalk.dim.italic(wrappedLine);
      }

      if (i < wrappedLines.length - 1) {
        outputBuffer += '\n';
      }
    }

    if (newlineIdx === -1) {
      buf.thinkingPendingNewline = true;
      // Single atomic write
      if (outputBuffer) {
        safeWrite(outputBuffer);
      }
      break;
    } else {
      outputBuffer += '\n';
      buf.thinkingNeedsPrefix = true;
      buf.thinkingPendingNewline = false;
      remaining = remaining.slice(newlineIdx + 1);
      // Single atomic write
      if (outputBuffer) {
        safeWrite(outputBuffer);
      }
    }
  }
}

// Flush pending content - just add newline if we have pending text
function flushLineBuffer(prefix, sender) {
  const buf = lineBuffers.get(sender);
  if (!buf) return;

  // CRITICAL: Batch all output into single safeWrite() to prevent interleaving with render()
  let outputBuffer = '';

  // Flush any remaining text in textBuffer (text without trailing newline)
  if (buf.textBuffer && buf.textBuffer.length > 0) {
    // Calculate widths for word wrapping (same as accumulateText)
    const prefixLen = chalk.reset(prefix).replace(/\\x1b\[[0-9;]*m/g, '').length + 1;
    const termWidth = getTerminalWidth();
    const contentWidth = Math.max(40, termWidth - prefixLen - 2);
    const continuationPrefix = ' '.repeat(prefixLen);

    const wrappedLines = wordWrap(buf.textBuffer, contentWidth);
    for (let i = 0; i < wrappedLines.length; i++) {
      const wrappedLine = wrappedLines[i];

      if (buf.needsPrefix) {
        outputBuffer += `${prefix} `;
        buf.needsPrefix = false;
      } else if (i > 0) {
        outputBuffer += `${continuationPrefix}`;
      }

      if (wrappedLine.trim()) {
        outputBuffer += formatInlineMarkdown(wrappedLine);
      }

      if (i < wrappedLines.length - 1) {
        outputBuffer += '\n';
      }
    }

    // Clear the buffer
    buf.textBuffer = '';
    buf.pendingNewline = true; // Mark that we need a newline before next prefix
  }

  if (buf.pendingNewline) {
    outputBuffer += '\n';
    buf.needsPrefix = true;
    buf.pendingNewline = false;
  }
  if (buf.thinkingPendingNewline) {
    outputBuffer += '\n';
    buf.thinkingNeedsPrefix = true;
    buf.thinkingPendingNewline = false;
  }

  // Single atomic write prevents interleaving
  if (outputBuffer) {
    safeWrite(outputBuffer);
  }
}

// Lines to filter out (noise, metadata, errors)
const FILTERED_PATTERNS = [
  // ct internal output
  /^--- Following log/,
  /--- Following logs/,
  /Ctrl\+C to stop/,
  /^=== Claude Task:/,
  /^Started:/,
  /^Finished:/,
  /^Exit code:/,
  /^CWD:/,
  /^={50}$/,
  // Agent context metadata
  /^Prompt: You are agent/,
  /^Iteration:/,
  /^## Triggering Message/,
  /^## Messages from topic:/,
  /^## Instructions/,
  /^## Output Format/,
  /^Topic: [A-Z_]+$/,
  /^Sender:/,
  /^Data: \{/,
  /^"issue_number"/,
  /^"title"/,
  /^"commit"/,
  /^\[\d{4}-\d{2}-\d{2}T/, // ISO timestamps
  /^# Manual Input$/,
  // Task errors (internal)
  /^Task not found:/,
  // JSON fragments
  /^\s*\{$/,
  /^\s*\}$/,
  /^\s*"[a-z_]+":.*,?\s*$/,
  // Template variables (unresolved)
  /\{\{[a-z.]+\}\}/,
];

// Helper function to print a message (docker-compose style with colors)
function printMessage(msg, showClusterId = false, watchMode = false, isActive = true) {
  // Build prefix using utility function
  const prefix = buildMessagePrefix(msg, showClusterId, isActive);

  const timestamp = new Date(msg.timestamp).toLocaleTimeString('en-US', {
    hour12: false,
  });

  // Watch mode: delegate to watch mode formatter
  if (watchMode) {
    const clusterPrefix = buildClusterPrefix(msg, isActive);
    formatWatchMode(msg, clusterPrefix);
    return;
  }

  // Normal mode: delegate to appropriate formatter based on topic
  if (msg.topic === 'AGENT_LIFECYCLE') {
    formatAgentLifecycle(msg, prefix);
    return;
  }

  if (msg.topic === 'AGENT_ERROR') {
    formatAgentErrorNormal(msg, prefix, timestamp);
    return;
  }

  if (msg.topic === 'ISSUE_OPENED') {
    formatIssueOpenedNormal(msg, prefix, timestamp, shownNewTaskForCluster);
    return;
  }

  if (msg.topic === 'IMPLEMENTATION_READY') {
    formatImplementationReadyNormal(msg, prefix, timestamp);
    return;
  }

  if (msg.topic === 'VALIDATION_RESULT') {
    formatValidationResultNormal(msg, prefix, timestamp);
    return;
  }

  if (msg.topic === 'PR_CREATED') {
    formatPrCreated(msg, prefix, timestamp, safePrint);
    return;
  }

  if (msg.topic === 'CLUSTER_COMPLETE') {
    formatClusterComplete(msg, prefix, timestamp, safePrint);
    return;
  }

  if (msg.topic === 'CLUSTER_FAILED') {
    formatClusterFailed(msg, prefix, timestamp, safePrint);
    return;
  }

  // AGENT_OUTPUT: handle separately (complex streaming logic - kept in main file due to dependencies)
  if (msg.topic === 'AGENT_OUTPUT') {
    // Support both old 'chunk' and new 'line' formats
    const content = msg.content?.data?.line || msg.content?.data?.chunk || msg.content?.text;
    if (!content || !content.trim()) return;

    // Parse streaming JSON events using the parser
    const events = parseChunk(content);

    for (const event of events) {
      switch (event.type) {
        case 'text':
          // Accumulate text, print complete lines
          accumulateText(prefix, msg.sender, event.text);
          break;

        case 'thinking':
        case 'thinking_start':
          // Accumulate thinking, print complete lines
          if (event.text) {
            accumulateThinking(prefix, msg.sender, event.text);
          } else if (event.type === 'thinking_start') {
            safePrint(`${prefix} ${chalk.dim.italic('💭 thinking...')}`);
          }
          break;

        case 'tool_start':
          // Flush pending text before tool - don't print, tool_call has details
          flushLineBuffer(prefix, msg.sender);
          break;

        case 'tool_call':
          // Flush pending text before tool
          flushLineBuffer(prefix, msg.sender);
          const icon = getToolIcon(event.toolName);
          const toolDesc = formatToolCall(event.toolName, event.input);
          safePrint(`${prefix} ${icon} ${chalk.cyan(event.toolName)} ${chalk.dim(toolDesc)}`);
          // Store tool call info for matching with result
          currentToolCall.set(msg.sender, {
            toolName: event.toolName,
            input: event.input,
          });
          break;

        case 'tool_input':
          // Streaming tool input JSON - skip (shown in tool_call)
          break;

        case 'tool_result':
          const status = event.isError ? chalk.red('✗') : chalk.green('✓');
          // Get stored tool call info for better formatting
          const toolCall = currentToolCall.get(msg.sender);
          const resultDesc = formatToolResult(
            event.content,
            event.isError,
            toolCall?.toolName,
            toolCall?.input
          );
          safePrint(`${prefix}   ${status} ${resultDesc}`);
          // Clear stored tool call after result
          currentToolCall.delete(msg.sender);
          break;

        case 'result':
          // Flush remaining buffer before result
          flushLineBuffer(prefix, msg.sender);
          // Final result - only show errors (success text already streamed)
          if (!event.success) {
            safePrint(`${prefix} ${chalk.bold.red('✗ Error:')} ${event.error || 'Task failed'}`);
          }
          break;

        case 'block_end':
          // Block ended - skip
          break;

        default:
          // Unknown event type - skip
          break;
      }
    }

    // If no JSON events parsed, fall through to text filtering
    if (events.length === 0) {
      const lines = content.split('\n');
      for (const line of lines) {
        const trimmed = line.trim();
        if (!trimmed) continue;

        // Check against filtered patterns
        let shouldSkip = false;
        for (const pattern of FILTERED_PATTERNS) {
          if (pattern.test(trimmed)) {
            shouldSkip = true;
            break;
          }
        }
        if (shouldSkip) continue;

        // Skip JSON-like content
        if (
          (trimmed.startsWith('{') && trimmed.endsWith('}')) ||
          (trimmed.startsWith('[') && trimmed.endsWith(']'))
        )
          continue;

        // Skip duplicate content
        if (isDuplicate(trimmed)) continue;

        safePrint(`${prefix} ${line}`);
      }
    }
    return;
  }

  // AGENT_ERROR: Show errors with visual prominence
  if (msg.topic === 'AGENT_ERROR') {
    safePrint(''); // Blank line before error
    safePrint(chalk.bold.red(`${'─'.repeat(60)}`));
    safePrint(`${prefix} ${chalk.gray(timestamp)} ${chalk.bold.red('🔴 AGENT ERROR')}`);
    if (msg.content?.text) {
      safePrint(`${prefix} ${chalk.red(msg.content.text)}`);
    }
    if (msg.content?.data?.stack) {
      // Show first 5 lines of stack trace
      const stackLines = msg.content.data.stack.split('\n').slice(0, 5);
      for (const line of stackLines) {
        if (line.trim()) {
          safePrint(`${prefix} ${chalk.dim(line)}`);
        }
      }
    }
    safePrint(chalk.bold.red(`${'─'.repeat(60)}`));
    return;
  }

  // ISSUE_OPENED: Show as task header with visual separation
  // Skip duplicate - conductor re-publishes after spawning agents (same task, confusing UX)
  if (msg.topic === 'ISSUE_OPENED') {
    if (shownNewTaskForCluster.has(msg.cluster_id)) {
      return; // Already shown NEW TASK for this cluster
    }
    shownNewTaskForCluster.add(msg.cluster_id);

    safePrint(''); // Blank line before new task
    safePrint(chalk.bold.blue(`${'─'.repeat(60)}`));
    safePrint(`${prefix} ${chalk.gray(timestamp)} ${chalk.bold.blue('📋 NEW TASK')}`);
    if (msg.content?.text) {
      // Show task description (first 3 lines max)
      const lines = msg.content.text.split('\n').slice(0, 3);
      for (const line of lines) {
        if (line.trim() && line.trim() !== '# Manual Input') {
          safePrint(`${prefix} ${chalk.white(line)}`);
        }
      }
    }
    safePrint(chalk.bold.blue(`${'─'.repeat(60)}`));
    return;
  }

  // IMPLEMENTATION_READY: milestone marker
  if (msg.topic === 'IMPLEMENTATION_READY') {
    safePrint(
      `${prefix} ${chalk.gray(timestamp)} ${chalk.bold.yellow('✅ IMPLEMENTATION READY')}`
    );
    if (msg.content?.data?.commit) {
      safePrint(
        `${prefix} ${chalk.gray('Commit:')} ${chalk.cyan(msg.content.data.commit.substring(0, 8))}`
      );
    }
    return;
  }

  // VALIDATION_RESULT: show approval/rejection clearly
  if (msg.topic === 'VALIDATION_RESULT') {
    const data = msg.content?.data || {};
    const approved = data.approved === true || data.approved === 'true';
    const status = approved ? chalk.bold.green('✓ APPROVED') : chalk.bold.red('✗ REJECTED');

    safePrint(`${prefix} ${chalk.gray(timestamp)} ${status}`);

    // Show summary if present and not a template variable
    if (msg.content?.text && !msg.content.text.includes('{{')) {
      safePrint(`${prefix} ${msg.content.text.substring(0, 100)}`);
    }

    // Show full JSON data structure
    safePrint(
      `${prefix} ${chalk.dim(JSON.stringify(data, null, 2).split('\n').join(`\n${prefix} `))}`
    );

    // Show errors/issues if any
    if (data.errors && Array.isArray(data.errors) && data.errors.length > 0) {
      safePrint(`${prefix} ${chalk.red('Errors:')}`);
      data.errors.forEach((err) => {
        if (err && typeof err === 'string') {
          safePrint(`${prefix}   - ${err}`);
        }
      });
    }

    if (data.issues && Array.isArray(data.issues) && data.issues.length > 0) {
      safePrint(`${prefix} ${chalk.yellow('Issues:')}`);
      data.issues.forEach((issue) => {
        if (issue && typeof issue === 'string') {
          safePrint(`${prefix}   - ${issue}`);
        }
      });
    }
    return;
  }

  // Fallback: generic message display for unknown topics
  formatGenericMessage(msg, prefix, timestamp, safePrint);
}

// Main async entry point
async function main() {
  // First-run setup wizard (blocks on first use only)
  const isQuiet = process.argv.includes('-q') || process.argv.includes('--quiet');
  await checkFirstRun({ quiet: isQuiet });

  // Check for updates (non-blocking if offline)
  await checkForUpdates({ quiet: isQuiet });

  // Default command handling: if first arg doesn't match a known command, treat it as 'run'
  // This allows `zeroshot "task"` to work the same as `zeroshot run "task"`
  const args = process.argv.slice(2);
  if (args.length > 0) {
    const firstArg = args[0];

    // Skip if it's a flag/option (starts with -)
    // Skip if it's --help or --version (these are handled by commander)
    if (!firstArg.startsWith('-')) {
      // Get all registered command names
      const commandNames = program.commands.map((cmd) => cmd.name());

      // If first arg is not a known command, prepend 'run'
      if (!commandNames.includes(firstArg)) {
        process.argv.splice(2, 0, 'run');
      }
    }
  }

  program.parse();
}

// Run main
main().catch((err) => {
  console.error('Fatal error:', err.message);
  process.exit(1);
});
