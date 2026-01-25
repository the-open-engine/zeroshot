#!/usr/bin/env node

/**
 * Attachable Watcher - spawns a CLI process with PTY for attach/detach support
 * Runs detached from parent, provides Unix socket for attach clients.
 */

import { appendFileSync, existsSync, mkdirSync } from 'fs';
import { join } from 'path';
import { homedir } from 'os';
import { updateTask } from './store.js';
import {
  detectFatalClaudeError,
  detectStreamingModeError,
  recoverStructuredOutput,
} from './claude-recovery.js';
import { createRequire } from 'module';

// ═══════════════════════════════════════════════════════════════════════════
// 🔴 CRITICAL: Global error handlers - MUST be installed BEFORE any async ops
// Without these, uncaught errors cause SILENT process death (no logs, no status)
// ═══════════════════════════════════════════════════════════════════════════

const [, , taskIdArg, cwdArg, logFileArg, argsJsonArg, configJsonArg] = process.argv;

function emergencyLog(msg) {
  if (logFileArg) {
    try {
      appendFileSync(logFileArg, msg);
    } catch {
      process.stderr.write(msg);
    }
  } else {
    process.stderr.write(msg);
  }
}

function crashWithError(error, source) {
  const timestamp = Date.now();
  const errorMsg = error instanceof Error ? error.stack || error.message : String(error);

  emergencyLog(`\n[${timestamp}][CRASH] ${source}: ${errorMsg}\n`);
  emergencyLog(`[${timestamp}][CRASH] Process terminating due to unhandled error\n`);

  if (taskIdArg) {
    try {
      updateTask(taskIdArg, {
        status: 'failed',
        error: `${source}: ${errorMsg}`,
        socketPath: null,
      });
    } catch (updateError) {
      emergencyLog(`[${timestamp}][CRASH] Failed to update task status: ${updateError.message}\n`);
    }
  }

  process.exit(1);
}

process.on('uncaughtException', (error) => {
  crashWithError(error, 'uncaughtException');
});

process.on('unhandledRejection', (reason) => {
  crashWithError(reason, 'unhandledRejection');
});

const require = createRequire(import.meta.url);
const { AttachServer } = require('../src/attach');
const { normalizeProviderName } = require('../lib/provider-names');

const taskId = taskIdArg;
const cwd = cwdArg;
const logFile = logFileArg;
const args = JSON.parse(argsJsonArg);
const config = configJsonArg ? JSON.parse(configJsonArg) : {};
let server = null;

const SOCKET_DIR = join(homedir(), '.zeroshot', 'sockets');
const socketPath = join(SOCKET_DIR, `${taskId}.sock`);

if (!existsSync(SOCKET_DIR)) {
  mkdirSync(SOCKET_DIR, { recursive: true });
}

function log(msg) {
  appendFileSync(logFile, msg);
}

const providerName = normalizeProviderName(config.provider || 'claude');
const enableRecovery = providerName === 'claude';

const env = { ...process.env, ...(config.env || {}) };
const command = config.command || 'claude';
const finalArgs = [...args];

const silentJsonMode =
  config.outputFormat === 'json' && config.jsonSchema && config.silentJsonOutput && enableRecovery;

let finalResultJson = null;
let outputBuffer = '';
let streamingModeError = null;
let fatalError = null;

function splitBufferLines(buffer, chunk) {
  const nextBuffer = buffer + chunk;
  const lines = nextBuffer.split('\n');
  const remaining = lines.pop() || '';
  return { lines, remaining };
}

function maybeHandleFatalError(line, timestamp) {
  if (!enableRecovery || fatalError) {
    return false;
  }

  const detected = detectFatalClaudeError(line);
  if (!detected) {
    return false;
  }

  fatalError = detected;

  if (silentJsonMode) {
    log(`[${timestamp}]${line}\n`);
  }
  log(`[${timestamp}][FATAL] ${detected}\n`);

  if (server) {
    server.stop('SIGTERM').catch(() => {});
  }
  return true;
}

function captureStreamingError(line, timestamp) {
  if (!enableRecovery) {
    return false;
  }

  const detectedError = detectStreamingModeError(line);
  if (!detectedError) {
    return false;
  }

  streamingModeError = { ...detectedError, timestamp };
  return true;
}

function maybeCaptureStructuredOutput(line) {
  try {
    const json = JSON.parse(line);
    if (json.structured_output) {
      finalResultJson = line;
    }
  } catch {
    // Not JSON, skip
  }
}

function handleSilentJsonLines(lines, timestamp) {
  for (const line of lines) {
    if (!line.trim()) continue;
    maybeHandleFatalError(line, timestamp);
    if (captureStreamingError(line, timestamp)) {
      continue;
    }
    maybeCaptureStructuredOutput(line);
  }
}

function handleStreamingLines(lines, timestamp) {
  for (const line of lines) {
    maybeHandleFatalError(line, timestamp);
    if (captureStreamingError(line, timestamp)) {
      continue;
    }
    log(`[${timestamp}]${line}\n`);
  }
}

function flushOutputBuffer(timestamp) {
  if (!outputBuffer.trim()) {
    return;
  }

  if (!enableRecovery) {
    if (!silentJsonMode) {
      log(`[${timestamp}]${outputBuffer}\n`);
    }
    return;
  }

  maybeHandleFatalError(outputBuffer, timestamp);
  if (captureStreamingError(outputBuffer, timestamp)) {
    return;
  }

  if (silentJsonMode) {
    maybeCaptureStructuredOutput(outputBuffer);
    return;
  }

  log(`[${timestamp}]${outputBuffer}\n`);
}

function attemptRecovery(code, timestamp) {
  if (!(enableRecovery && code !== 0 && streamingModeError?.sessionId)) {
    return null;
  }

  const recovered = recoverStructuredOutput(streamingModeError.sessionId);
  if (recovered?.payload) {
    const recoveredLine = JSON.stringify(recovered.payload);
    if (silentJsonMode) {
      finalResultJson = recoveredLine;
    } else {
      log(`[${timestamp}]${recoveredLine}\n`);
    }
  } else if (streamingModeError.line) {
    if (silentJsonMode) {
      log(streamingModeError.line + '\n');
    } else {
      log(`[${streamingModeError.timestamp}]${streamingModeError.line}\n`);
    }
  }

  return recovered;
}

function writeCompletionFooter(code, signal) {
  if (config.outputFormat === 'json') {
    return;
  }

  log(`\n${'='.repeat(50)}\n`);
  log(`Finished: ${new Date().toISOString()}\n`);
  log(`Exit code: ${code}, Signal: ${signal}\n`);
}

server = new AttachServer({
  id: taskId,
  socketPath,
  command,
  args: finalArgs,
  cwd,
  env,
  cols: 120,
  rows: 30,
});

server.on('output', (data) => {
  const chunk = data.toString();
  const timestamp = Date.now();

  const { lines, remaining } = splitBufferLines(outputBuffer, chunk);
  outputBuffer = remaining;

  if (silentJsonMode) {
    handleSilentJsonLines(lines, timestamp);
  } else {
    handleStreamingLines(lines, timestamp);
  }
});

server.on('exit', async ({ exitCode, signal }) => {
  const timestamp = Date.now();
  const code = exitCode;

  flushOutputBuffer(timestamp);

  const recovered = attemptRecovery(code, timestamp);

  if (silentJsonMode && finalResultJson) {
    log(finalResultJson + '\n');
  }

  writeCompletionFooter(code, signal);

  const resolvedCode = fatalError ? 1 : recovered?.payload ? 0 : code;
  const status = resolvedCode === 0 ? 'completed' : 'failed';
  try {
    await updateTask(taskId, {
      status,
      exitCode: resolvedCode,
      error: fatalError || (resolvedCode !== 0 && signal ? `Killed by ${signal}` : null),
      socketPath: null,
    });
  } catch (updateError) {
    log(`[${Date.now()}][ERROR] Failed to update task status: ${updateError.message}\n`);
  }

  setTimeout(() => {
    process.exit(0);
  }, 500);
});

server.on('error', async (err) => {
  log(`\nError: ${err.message}\n`);
  try {
    await updateTask(taskId, { status: 'failed', error: err.message });
  } catch (updateError) {
    log(`[${Date.now()}][ERROR] Failed to update task status: ${updateError.message}\n`);
  }
  process.exit(1);
});

server.on('clientAttach', ({ clientId }) => {
  log(`[${Date.now()}][ATTACH] Client attached: ${clientId.slice(0, 8)}...\n`);
});

server.on('clientDetach', ({ clientId }) => {
  log(`[${Date.now()}][DETACH] Client detached: ${clientId.slice(0, 8)}...\n`);
});

try {
  await server.start();

  updateTask(taskId, {
    pid: server.pid,
    socketPath,
    attachable: true,
  });

  log(`[${Date.now()}][SYSTEM] Started with PTY (attachable)\n`);
  log(`[${Date.now()}][SYSTEM] Socket: ${socketPath}\n`);
  log(`[${Date.now()}][SYSTEM] PID: ${server.pid}\n`);
} catch (err) {
  log(`\nFailed to start: ${err.message}\n`);
  updateTask(taskId, { status: 'failed', error: err.message });
  process.exit(1);
}

process.on('SIGTERM', async () => {
  log(`[${Date.now()}][SYSTEM] Received SIGTERM, stopping...\n`);
  await server.stop('SIGTERM');
});

process.on('SIGINT', async () => {
  log(`[${Date.now()}][SYSTEM] Received SIGINT, stopping...\n`);
  await server.stop('SIGINT');
});
