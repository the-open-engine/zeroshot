#!/usr/bin/env node

/**
 * Watcher process - spawns and monitors a claude process
 * Runs detached from parent, updates task status on completion
 *
 * Uses regular spawn (not PTY) - Claude CLI with --print is non-interactive
 * PTY causes EIO errors when processes are killed/OOM'd
 */

import { spawn } from 'child_process';
import { appendFileSync } from 'fs';
import { dirname } from 'path';
import { fileURLToPath } from 'url';
import { updateTask } from './store.js';
import { createRequire } from 'module';

const require = createRequire(import.meta.url);
const { getClaudeCommand } = require('../lib/settings.js');

const __dirname = dirname(fileURLToPath(import.meta.url));

const [, , taskId, cwd, logFile, argsJson, configJson] = process.argv;
const args = JSON.parse(argsJson);
const config = configJson ? JSON.parse(configJson) : {};

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

// Spawn claude using regular child_process (not PTY)
// --print mode is non-interactive, PTY adds overhead and causes EIO on OOM
const child = spawn(claudeCommand, finalArgs, {
  cwd,
  env,
  stdio: ['ignore', 'pipe', 'pipe'],
});

// Update task with PID
updateTask(taskId, { pid: child.pid });

// For JSON schema output with silent mode, capture ONLY the structured_output JSON
const silentJsonMode =
  config.outputFormat === 'json' && config.jsonSchema && config.silentJsonOutput;
let finalResultJson = null;

// Buffer for incomplete lines (need complete lines to add timestamps)
let stdoutBuffer = '';

// Process stdout data
// CRITICAL: Prepend timestamp to each line for real-time tracking in cluster
// Format: [1733301234567]{json...} - consumers parse timestamp for accurate timing
child.stdout.on('data', (data) => {
  const chunk = data.toString();
  const timestamp = Date.now();

  if (silentJsonMode) {
    // Parse each line to find the one with structured_output
    stdoutBuffer += chunk;
    const lines = stdoutBuffer.split('\n');
    stdoutBuffer = lines.pop() || ''; // Keep incomplete line in buffer

    for (const line of lines) {
      if (!line.trim()) continue;
      try {
        const json = JSON.parse(line);
        if (json.structured_output) {
          finalResultJson = line;
        }
      } catch {
        // Not JSON or incomplete, skip
      }
    }
  } else {
    // Normal mode - stream with timestamps on each complete line
    stdoutBuffer += chunk;
    const lines = stdoutBuffer.split('\n');
    stdoutBuffer = lines.pop() || ''; // Keep incomplete line in buffer

    for (const line of lines) {
      // Timestamp each line: [epochMs]originalContent
      log(`[${timestamp}]${line}\n`);
    }
  }
});

// Buffer for stderr incomplete lines
let stderrBuffer = '';

// Stream stderr to log with timestamps
child.stderr.on('data', (data) => {
  const chunk = data.toString();
  const timestamp = Date.now();

  stderrBuffer += chunk;
  const lines = stderrBuffer.split('\n');
  stderrBuffer = lines.pop() || '';

  for (const line of lines) {
    log(`[${timestamp}]${line}\n`);
  }
});

// Handle process exit
child.on('close', (code, signal) => {
  const timestamp = Date.now();

  // Flush any remaining buffered stdout
  if (stdoutBuffer.trim()) {
    if (silentJsonMode) {
      try {
        const json = JSON.parse(stdoutBuffer);
        if (json.structured_output) {
          finalResultJson = stdoutBuffer;
        }
      } catch {
        // Not valid JSON
      }
    } else {
      log(`[${timestamp}]${stdoutBuffer}\n`);
    }
  }

  // Flush any remaining buffered stderr
  if (stderrBuffer.trim()) {
    log(`[${timestamp}]${stderrBuffer}\n`);
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
  });
  process.exit(0);
});

child.on('error', (err) => {
  log(`\nError: ${err.message}\n`);
  updateTask(taskId, { status: 'failed', error: err.message });
  process.exit(1);
});
