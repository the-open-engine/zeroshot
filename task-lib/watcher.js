#!/usr/bin/env node

/**
 * Watcher process - spawns and monitors a CLI process
 * Runs detached from parent, updates task status on completion
 */

import { spawn } from 'child_process';
import { appendFileSync } from 'fs';
import { updateTask } from './store.js';
import {
  detectFatalClaudeError,
  detectStreamingModeError,
  recoverStructuredOutput,
} from './claude-recovery.js';
import { createRequire } from 'module';

const require = createRequire(import.meta.url);
const { normalizeProviderName } = require('../lib/provider-names');

const [, , taskId, cwd, logFile, argsJson, configJson] = process.argv;
const args = JSON.parse(argsJson);
const config = configJson ? JSON.parse(configJson) : {};

function log(msg) {
  appendFileSync(logFile, msg);
}

const providerName = normalizeProviderName(config.provider || 'claude');
const enableRecovery = providerName === 'claude';

const env = { ...process.env, ...(config.env || {}) };
const command = config.command || 'claude';
const finalArgs = [...args];

const child = spawn(command, finalArgs, {
  cwd,
  env,
  stdio: ['ignore', 'pipe', 'pipe'],
});

updateTask(taskId, { pid: child.pid });

const silentJsonMode =
  config.outputFormat === 'json' && config.jsonSchema && config.silentJsonOutput && enableRecovery;

let finalResultJson = null;
let streamingModeError = null;
let fatalError = null;

let stdoutBuffer = '';
let stderrBuffer = '';

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

  try {
    child.kill('SIGTERM');
  } catch {
    // Ignore - process may already be dead
  }

  setTimeout(() => {
    if (child.exitCode === null) {
      try {
        child.kill('SIGKILL');
      } catch {
        // Ignore - process may already be dead
      }
    }
  }, 5000);

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

function flushStdoutBuffer(timestamp) {
  if (!stdoutBuffer.trim()) {
    return;
  }

  if (!enableRecovery) {
    if (!silentJsonMode) {
      log(`[${timestamp}]${stdoutBuffer}\n`);
    }
    return;
  }

  maybeHandleFatalError(stdoutBuffer, timestamp);
  if (captureStreamingError(stdoutBuffer, timestamp)) {
    return;
  }

  if (silentJsonMode) {
    maybeCaptureStructuredOutput(stdoutBuffer);
    return;
  }

  log(`[${timestamp}]${stdoutBuffer}\n`);
}

function flushStderrBuffer(timestamp) {
  if (stderrBuffer.trim()) {
    maybeHandleFatalError(stderrBuffer, timestamp);
    log(`[${timestamp}]${stderrBuffer}\n`);
  }
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

child.stdout.on('data', (data) => {
  const chunk = data.toString();
  const timestamp = Date.now();

  const { lines, remaining } = splitBufferLines(stdoutBuffer, chunk);
  stdoutBuffer = remaining;

  if (silentJsonMode) {
    handleSilentJsonLines(lines, timestamp);
  } else {
    handleStreamingLines(lines, timestamp);
  }
});

child.stderr.on('data', (data) => {
  const chunk = data.toString();
  const timestamp = Date.now();

  const { lines, remaining } = splitBufferLines(stderrBuffer, chunk);
  stderrBuffer = remaining;

  for (const line of lines) {
    log(`[${timestamp}]${line}\n`);
  }
});

child.on('close', async (code, signal) => {
  const timestamp = Date.now();

  flushStdoutBuffer(timestamp);
  flushStderrBuffer(timestamp);

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
    });
  } catch (updateError) {
    log(`[${Date.now()}][ERROR] Failed to update task status: ${updateError.message}\n`);
  }
  process.exit(0);
});

child.on('error', async (err) => {
  log(`\nError: ${err.message}\n`);
  try {
    await updateTask(taskId, { status: 'failed', error: err.message });
  } catch (updateError) {
    log(`[${Date.now()}][ERROR] Failed to update task status: ${updateError.message}\n`);
  }
  process.exit(1);
});
