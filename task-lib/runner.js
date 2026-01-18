import { fork } from 'child_process';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { LOGS_DIR } from './config.js';
import { addTask, generateId, ensureDirs } from './store.js';
import { createRequire } from 'module';

const require = createRequire(import.meta.url);
const { loadSettings } = require('../lib/settings.js');
const { normalizeProviderName } = require('../lib/provider-names');
const { getProvider } = require('../src/providers');

const __dirname = dirname(fileURLToPath(import.meta.url));

export async function spawnTask(prompt, options = {}) {
  ensureDirs();

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
  const watcherScript = resolveWatcherScript(options);
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
    const promptIndex = finalArgs.length - 1;
    if (options.resume) {
      finalArgs.splice(promptIndex, 0, '--resume', options.resume);
    } else if (options.continue) {
      finalArgs.splice(promptIndex, 0, '--continue');
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
  };
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
