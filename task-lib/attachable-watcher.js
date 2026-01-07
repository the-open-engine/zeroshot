#!/usr/bin/env node

/**
 * Attachable Watcher - spawns Claude with PTY for attach/detach support
 *
 * Runs detached from parent, provides Unix socket for attach clients.
 * Uses node-pty for proper terminal emulation.
 *
 * Key differences from legacy watcher.js:
 * - Uses AttachServer (node-pty) instead of child_process.spawn
 * - Creates Unix socket at ~/.zeroshot/sockets/task-<id>.sock
 * - Supports multiple attached clients
 * - Still writes to log file for backward compatibility
 *
 * CRITICAL: Global error handlers installed FIRST to catch silent crashes
 */

import { appendFileSync, existsSync, mkdirSync } from 'fs';
import { join } from 'path';
import { homedir } from 'os';
import { updateTask } from './store.js';

// ═══════════════════════════════════════════════════════════════════════════
// 🔴 CRITICAL: Global error handlers - MUST be installed BEFORE any async ops
// Without these, uncaught errors cause SILENT process death (no logs, no status)
// ═══════════════════════════════════════════════════════════════════════════

// Parse args early so we can log errors to the correct file
const [, , taskIdArg, cwdArg, logFileArg, argsJsonArg, configJsonArg] = process.argv;

/**
 * Emergency logger - works even if main log function isn't ready
 */
function emergencyLog(msg) {
  if (logFileArg) {
    try {
      appendFileSync(logFileArg, msg);
    } catch {
      // Last resort - stderr
      process.stderr.write(msg);
    }
  } else {
    process.stderr.write(msg);
  }
}

/**
 * Mark task as failed and exit
 */
function crashWithError(error, source) {
  const timestamp = Date.now();
  const errorMsg = error instanceof Error ? error.stack || error.message : String(error);

  emergencyLog(`\n[${timestamp}][CRASH] ${source}: ${errorMsg}\n`);
  emergencyLog(`[${timestamp}][CRASH] Process terminating due to unhandled error\n`);

  // Try to update task status - may fail if error is in store.js itself
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

  // Exit with error code
  process.exit(1);
}

// Install handlers IMMEDIATELY
process.on('uncaughtException', (error) => {
  crashWithError(error, 'uncaughtException');
});

process.on('unhandledRejection', (reason) => {
  crashWithError(reason, 'unhandledRejection');
});

// Import attach infrastructure from src package (CommonJS)
import { createRequire } from 'module';
const require = createRequire(import.meta.url);
const { AttachServer } = require('../src/attach');
const { getClaudeCommand } = require('../lib/settings.js');

// Use the args parsed earlier (during error handler setup)
const taskId = taskIdArg;
const cwd = cwdArg;
const logFile = logFileArg;
const args = JSON.parse(argsJsonArg);
const config = configJsonArg ? JSON.parse(configJsonArg) : {};

// Socket path for attach
const SOCKET_DIR = join(homedir(), '.zeroshot', 'sockets');
const socketPath = join(SOCKET_DIR, `${taskId}.sock`);

// Ensure socket directory exists
if (!existsSync(SOCKET_DIR)) {
  mkdirSync(SOCKET_DIR, { recursive: true });
}

function log(msg) {
  appendFileSync(logFile, msg);
}

// Build environment - inherit user's auth method (API key or subscription)
const env = { ...process.env };

// Add model flag - priority: config.model > ANTHROPIC_MODEL env var
const claudeArgs = [...args];
const model = config.model || env.ANTHROPIC_MODEL;
if (model && !claudeArgs.includes('--model')) {
  claudeArgs.unshift('--model', model);
}

// Get configured Claude command (supports custom commands like 'ccr code')
const { command: claudeCommand, args: claudeExtraArgs } = getClaudeCommand();
const finalArgs = [...claudeExtraArgs, ...claudeArgs];

// For JSON schema output with silent mode, track final result
const silentJsonMode =
  config.outputFormat === 'json' && config.jsonSchema && config.silentJsonOutput;
let finalResultJson = null;

// Buffer for incomplete lines
let outputBuffer = '';

// Create AttachServer to spawn Claude with PTY
const server = new AttachServer({
  id: taskId,
  socketPath,
  command: claudeCommand,
  args: finalArgs,
  cwd,
  env,
  cols: 120,
  rows: 30,
});

// Handle output from PTY
server.on('output', (data) => {
  const chunk = data.toString();
  const timestamp = Date.now();

  if (silentJsonMode) {
    // Parse each line to find structured_output
    outputBuffer += chunk;
    const lines = outputBuffer.split('\n');
    outputBuffer = lines.pop() || '';

    for (const line of lines) {
      if (!line.trim()) continue;
      try {
        const json = JSON.parse(line);
        if (json.structured_output) {
          finalResultJson = line;
        }
      } catch {
        // Not JSON, skip
      }
    }
  } else {
    // Normal mode - stream with timestamps
    outputBuffer += chunk;
    const lines = outputBuffer.split('\n');
    outputBuffer = lines.pop() || '';

    for (const line of lines) {
      log(`[${timestamp}]${line}\n`);
    }
  }
});

// Handle process exit
server.on('exit', ({ exitCode, signal }) => {
  const timestamp = Date.now();
  const code = exitCode;

  // Flush remaining buffered output
  if (outputBuffer.trim()) {
    if (silentJsonMode) {
      try {
        const json = JSON.parse(outputBuffer);
        if (json.structured_output) {
          finalResultJson = outputBuffer;
        }
      } catch {
        // Not valid JSON
      }
    } else {
      log(`[${timestamp}]${outputBuffer}\n`);
    }
  }

  // In silent JSON mode, log ONLY the final structured_output JSON
  if (silentJsonMode && finalResultJson) {
    log(finalResultJson + '\n');
  }

  // Skip footer for pure JSON output
  if (config.outputFormat !== 'json') {
    log(`\n${'='.repeat(50)}\n`);
    log(`Finished: ${new Date().toISOString()}\n`);
    log(`Exit code: ${code}, Signal: ${signal}\n`);
  }

  // Simple status: completed if exit 0, failed otherwise
  const status = code === 0 ? 'completed' : 'failed';
  updateTask(taskId, {
    status,
    exitCode: code,
    error: signal ? `Killed by ${signal}` : null,
    socketPath: null, // Clear socket path on exit
  });

  // Give clients time to receive exit message before exiting
  setTimeout(() => {
    process.exit(0);
  }, 500);
});

// Handle errors
server.on('error', (err) => {
  log(`\nError: ${err.message}\n`);
  updateTask(taskId, { status: 'failed', error: err.message });
  process.exit(1);
});

// Handle client attach/detach for logging
server.on('clientAttach', ({ clientId }) => {
  log(`[${Date.now()}][ATTACH] Client attached: ${clientId.slice(0, 8)}...\n`);
});

server.on('clientDetach', ({ clientId }) => {
  log(`[${Date.now()}][DETACH] Client detached: ${clientId.slice(0, 8)}...\n`);
});

// Start the server
try {
  await server.start();

  // Update task with PID and socket path
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

// Handle process signals for cleanup
process.on('SIGTERM', async () => {
  log(`[${Date.now()}][SYSTEM] Received SIGTERM, stopping...\n`);
  await server.stop('SIGTERM');
});

process.on('SIGINT', async () => {
  log(`[${Date.now()}][SYSTEM] Received SIGINT, stopping...\n`);
  await server.stop('SIGINT');
});
