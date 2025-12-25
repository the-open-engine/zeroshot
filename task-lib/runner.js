import { fork } from 'child_process';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { LOGS_DIR, DEFAULT_MODEL } from './config.js';
import { addTask, generateId, ensureDirs } from './store.js';

const __dirname = dirname(fileURLToPath(import.meta.url));

export function spawnTask(prompt, options = {}) {
  ensureDirs();

  const id = generateId();
  const logFile = join(LOGS_DIR, `${id}.log`);
  const cwd = options.cwd || process.cwd();
  const model = options.model || process.env.ANTHROPIC_MODEL || DEFAULT_MODEL;

  // Build claude command args
  // --print: non-interactive mode
  // --dangerously-skip-permissions: background tasks can't prompt for approval (CRITICAL)
  // --output-format: stream-json (default) for real-time, text for clean output, json for structured
  const outputFormat = options.outputFormat || 'stream-json';
  const args = ['--print', '--dangerously-skip-permissions', '--output-format', outputFormat];

  // Only add streaming options for stream-json format
  if (outputFormat === 'stream-json') {
    args.push('--verbose');
    // Include partial messages to get streaming updates before completion (required for stream-json format)
    args.push('--include-partial-messages');
  }

  // Add JSON schema if provided (only works with --output-format json)
  if (options.jsonSchema) {
    if (outputFormat !== 'json') {
      console.warn('Warning: --json-schema requires --output-format json, ignoring schema');
    } else {
      // CRITICAL: Must stringify schema object before passing to CLI (like zeroshot does)
      const schemaString =
        typeof options.jsonSchema === 'string'
          ? options.jsonSchema
          : JSON.stringify(options.jsonSchema);
      args.push('--json-schema', schemaString);
    }
  }

  if (options.resume) {
    args.push('--resume', options.resume);
  } else if (options.continue) {
    args.push('--continue');
  }

  args.push(prompt);

  const task = {
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
    // Schedule reference (if spawned by scheduler)
    scheduleId: options.scheduleId || null,
    // Attach support
    socketPath: null,
    attachable: false,
  };

  addTask(task);

  // Fork a watcher process that will manage the claude process
  const watcherConfig = {
    outputFormat,
    jsonSchema: options.jsonSchema || null,
    silentJsonOutput: options.silentJsonOutput || false,
    model,
  };

  // Use attachable watcher by default (unless explicitly disabled)
  // Attachable watcher uses node-pty and creates a Unix socket for attach/detach
  const useAttachable = options.attachable !== false;
  const watcherScript = useAttachable
    ? join(__dirname, 'attachable-watcher.js')
    : join(__dirname, 'watcher.js');

  const watcher = fork(
    watcherScript,
    [id, cwd, logFile, JSON.stringify(args), JSON.stringify(watcherConfig)],
    {
      detached: true,
      stdio: 'ignore',
    }
  );

  watcher.unref();

  // Return task immediately - watcher will update PID async
  return task;
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
