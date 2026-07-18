import { fork } from 'child_process';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { LOGS_DIR } from './config.js';
import { addTask, generateId, ensureDirs } from './store.js';
import { createRequire } from 'module';

const require = createRequire(import.meta.url);
const { prepareSingleAgentProviderCommand } = require('./provider-helper-runtime.js');

const __dirname = dirname(fileURLToPath(import.meta.url));

export function spawnTask(prompt, options = {}) {
  ensureDirs();

  const id = generateId();
  const logFile = join(LOGS_DIR, `${id}.log`);
  const cwd = options.cwd || process.cwd();

  const outputFormat = resolveOutputFormat(options);
  const jsonSchema = resolveJsonSchema(options, outputFormat);
  const prepared = prepareSingleAgentProviderCommand({
    provider: options.provider || null,
    context: prompt,
    options: buildProviderOptions(options, outputFormat, jsonSchema, cwd),
  });
  const providerName = prepared.adapter.id;
  const modelSpec = prepared.options.modelSpec;
  const commandSpec = prepared.commandSpec;

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
  const watcherScript = resolveWatcherScript(options);
  spawnWatcher({
    watcherScript,
    id,
    cwd,
    logFile,
    finalArgs: commandSpec.args,
    watcherConfig,
  });

  return task;
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

function buildProviderOptions(options, outputFormat, jsonSchema, cwd) {
  return {
    outputFormat,
    jsonSchema,
    cwd,
    autoApprove: true,
    ...modelSpecOption(options),
    ...mcpConfigOption(options),
    ...(options.resume ? { resumeSessionId: options.resume } : {}),
    ...(options.continue ? { continueSession: true } : {}),
  };
}

function mcpConfigOption(options) {
  const entries = options.mcpConfig;
  if (!Array.isArray(entries) || entries.length === 0) return {};
  return { mcpConfig: entries };
}

function modelSpecOption(options) {
  const modelSpec = resolveRequestedModelSpec(options);
  return modelSpec === undefined ? {} : { modelSpec };
}

function resolveRequestedModelSpec(options) {
  if (options.model) {
    return {
      model: options.model,
      ...(options.reasoningEffort ? { reasoningEffort: options.reasoningEffort } : {}),
    };
  }

  if (options.reasoningEffort) {
    return {
      ...(options.modelLevel ? { level: options.modelLevel } : {}),
      reasoningEffort: options.reasoningEffort,
    };
  }
  if (options.modelLevel) return { level: options.modelLevel };
  return undefined;
}

function buildTaskRecord({ id, prompt, cwd, options, logFile, providerName, modelSpec }) {
  return {
    id,
    prompt: prompt.slice(0, 200) + (prompt.length > 200 ? '...' : ''),
    fullPrompt: prompt,
    cwd,
    status: 'running',
    pid: null,
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
    commandSpec: buildWatcherCommandSpec(commandSpec),
  };
}

function buildWatcherCommandSpec(commandSpec) {
  const watcherCommandSpec = { ...commandSpec };
  delete watcherCommandSpec.args;
  return watcherCommandSpec;
}

function resolveWatcherScript(options) {
  const useAttachable = options.attachable !== false && !options.jsonSchema;
  return useAttachable ? join(__dirname, 'attachable-watcher.js') : join(__dirname, 'watcher.js');
}

function spawnWatcher({ watcherScript, id, cwd, logFile, finalArgs, watcherConfig }) {
  const watcher = fork(
    watcherScript,
    [id, cwd, logFile, JSON.stringify(finalArgs), JSON.stringify(watcherConfig)],
    {
      detached: true,
      stdio: 'ignore',
      windowsHide: true,
    }
  );

  watcher.unref();
  watcher.disconnect(); // Close IPC channel so parent can exit
}

export function isProcessRunning(pid) {
  if (!pid) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

export function killTask(pid) {
  if (!pid) return false;
  try {
    process.kill(pid, 'SIGTERM');
    return true;
  } catch {
    return false;
  }
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitForProcessExit(pid, timeoutMs, pollMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (!isProcessRunning(pid)) return true;
    await sleep(pollMs);
  }
  return !isProcessRunning(pid);
}

/**
 * Terminate a provider process without relying on Linux-only process metadata.
 * SIGTERM gets a bounded grace period, then SIGKILL is used as the final
 * authority so callers never leave a task persisted as running indefinitely.
 */
export async function terminateProcess(pid, options = {}) {
  const graceMs = options.graceMs ?? 5000;
  const hardKillWaitMs = options.hardKillWaitMs ?? 1000;
  const pollMs = options.pollMs ?? 50;

  if (!isProcessRunning(pid)) {
    return {
      terminated: true,
      alreadyDead: true,
      escalated: false,
      signal: null,
    };
  }

  try {
    process.kill(pid, 'SIGTERM');
  } catch (error) {
    if (!isProcessRunning(pid)) {
      return {
        terminated: true,
        alreadyDead: true,
        escalated: false,
        signal: null,
      };
    }
    return {
      terminated: false,
      alreadyDead: false,
      escalated: false,
      signal: 'SIGTERM',
      error: error.message,
    };
  }

  if (await waitForProcessExit(pid, graceMs, pollMs)) {
    return {
      terminated: true,
      alreadyDead: false,
      escalated: false,
      signal: 'SIGTERM',
    };
  }

  try {
    process.kill(pid, 'SIGKILL');
  } catch (error) {
    if (!isProcessRunning(pid)) {
      return {
        terminated: true,
        alreadyDead: false,
        escalated: true,
        signal: 'SIGKILL',
      };
    }
    return {
      terminated: false,
      alreadyDead: false,
      escalated: true,
      signal: 'SIGKILL',
      error: error.message,
    };
  }

  const terminated = await waitForProcessExit(pid, hardKillWaitMs, pollMs);
  return {
    terminated,
    alreadyDead: false,
    escalated: true,
    signal: 'SIGKILL',
    error: terminated ? null : `Process ${pid} survived SIGKILL`,
  };
}
