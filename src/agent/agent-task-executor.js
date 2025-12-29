// @ts-nocheck
/**
 * AgentTaskExecutor - Claude CLI spawning and monitoring
 *
 * Provides:
 * - Claude CLI task spawning (normal and isolated modes)
 * - Log streaming and real-time output broadcasting
 * - Task lifecycle management (wait, kill)
 * - Output parsing and validation
 * - Vibe-specific Claude config with AskUserQuestion blocked
 */

const { spawn } = require('child_process');
const path = require('path');
const fs = require('fs');
const os = require('os');

/**
 * Validate and sanitize error messages.
 * Detects TypeScript type annotations that may have leaked into error storage.
 *
 * @param {string|null} error - Error message to validate
 * @returns {string|null} Sanitized error or original if valid
 */
function sanitizeErrorMessage(error) {
  if (!error) return null;

  // Patterns that look like TypeScript type annotations (not real error messages)
  const typeAnnotationPatterns = [
    /^string\s*\|\s*null$/i,
    /^number\s*\|\s*undefined$/i,
    /^boolean\s*\|\s*null$/i,
    /^any$/i,
    /^unknown$/i,
    /^void$/i,
    /^never$/i,
    /^[A-Z][a-zA-Z]*\s*\|\s*(null|undefined)$/, // e.g., "Error | null"
    /^[a-z]+(\s*\|\s*[a-z]+)+$/i, // e.g., "string | number | boolean"
  ];

  for (const pattern of typeAnnotationPatterns) {
    if (pattern.test(error.trim())) {
      console.warn(
        `[agent-task-executor] WARNING: Error message looks like a TypeScript type annotation: "${error}". ` +
          `This indicates corrupted data. Replacing with generic error.`
      );
      return `Task failed with corrupted error data (original: "${error}")`;
    }
  }

  return error;
}

/**
 * Strip timestamp prefix from log lines.
 * Log lines may have format: [epochMs]{json...} or [epochMs]text
 *
 * @param {string} line - Raw log line
 * @returns {string} Line content without timestamp prefix, empty string for invalid input
 */
function stripTimestampPrefix(line) {
  if (!line || typeof line !== 'string') return '';
  const trimmed = line.trim().replace(/\r$/, '');
  if (!trimmed) return '';
  const match = trimmed.match(/^\[(\d{13})\](.*)$/);
  return match ? match[2] : trimmed;
}

/**
 * Extract error context from task output.
 * Shared by both isolated and non-isolated modes.
 *
 * @param {Object} params - Extraction parameters
 * @param {string} params.output - Full task output
 * @param {string} [params.statusOutput] - Status command output (non-isolated only)
 * @param {string} params.taskId - Task ID for error messages
 * @param {boolean} [params.isNotFound=false] - True if task was not found
 * @returns {string|null} Sanitized error context or null if extraction failed
 */
function extractErrorContext({ output, statusOutput, taskId, isNotFound = false }) {
  // Task not found - explicit error
  if (isNotFound) {
    return sanitizeErrorMessage(`Task ${taskId} not found (may have crashed or been killed)`);
  }

  // Try status output first (only available in non-isolated mode)
  if (statusOutput) {
    const statusErrorMatch = statusOutput.match(/Error:\s*(.+)/);
    if (statusErrorMatch) {
      return sanitizeErrorMessage(statusErrorMatch[1].trim());
    }
  }

  // Fall back to extracting from output (last 500 chars)
  const lastOutput = (output || '').slice(-500).trim();
  if (!lastOutput) {
    return sanitizeErrorMessage('Task failed with no output (check if task was interrupted or timed out)');
  }

  // Common error patterns
  const errorPatterns = [
    /Error:\s*(.+)/i,
    /error:\s*(.+)/i,
    /failed:\s*(.+)/i,
    /Exception:\s*(.+)/i,
    /panic:\s*(.+)/i,
  ];

  for (const pattern of errorPatterns) {
    const match = lastOutput.match(pattern);
    if (match) {
      return sanitizeErrorMessage(match[1].slice(0, 200));
    }
  }

  // No pattern matched - include last portion of output
  return sanitizeErrorMessage(`Task failed. Last output: ${lastOutput.slice(-200)}`);
}

// Track if we've already ensured the AskUserQuestion hook is installed
let askUserQuestionHookInstalled = false;

// Track if we've already ensured the dangerous git hook is installed
let dangerousGitHookInstalled = false;

/**
 * Extract token usage from NDJSON output.
 * Looks for the 'result' event line which contains usage data.
 *
 * @param {string} output - Full NDJSON output from Claude CLI
 * @returns {Object|null} Token usage data or null if not found
 */
function extractTokenUsage(output) {
  if (!output) return null;

  const lines = output.split('\n');

  // Find the result line containing usage data
  for (const line of lines) {
    const content = stripTimestampPrefix(line);
    if (!content) continue;

    try {
      const event = JSON.parse(content);
      if (event.type === 'result') {
        const usage = event.usage || {};
        return {
          inputTokens: usage.input_tokens || 0,
          outputTokens: usage.output_tokens || 0,
          cacheReadInputTokens: usage.cache_read_input_tokens || 0,
          cacheCreationInputTokens: usage.cache_creation_input_tokens || 0,
          totalCostUsd: event.total_cost_usd || null,
          durationMs: event.duration_ms || null,
          modelUsage: event.modelUsage || null,
        };
      }
    } catch {
      // Not valid JSON, continue
    }
  }

  return null;
}

/**
 * Ensure the AskUserQuestion blocking hook is installed in user's Claude config.
 * This adds defense-in-depth by blocking the tool at the Claude CLI level.
 * Modifies ~/.claude/settings.json and copies hook script to ~/.claude/hooks/
 *
 * Safe to call multiple times - only modifies config once per process.
 */
function ensureAskUserQuestionHook() {
  if (askUserQuestionHookInstalled) {
    return; // Already installed this session
  }

  const userClaudeDir = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude');
  const hooksDir = path.join(userClaudeDir, 'hooks');
  const settingsPath = path.join(userClaudeDir, 'settings.json');
  const hookScriptName = 'block-ask-user-question.py';
  const hookScriptDst = path.join(hooksDir, hookScriptName);

  // Ensure hooks directory exists
  if (!fs.existsSync(hooksDir)) {
    fs.mkdirSync(hooksDir, { recursive: true });
  }

  // Copy hook script if not present or outdated
  const hookScriptSrc = path.join(__dirname, '..', '..', 'hooks', hookScriptName);
  if (fs.existsSync(hookScriptSrc)) {
    // Always copy to ensure latest version
    fs.copyFileSync(hookScriptSrc, hookScriptDst);
    fs.chmodSync(hookScriptDst, 0o755);
  }

  // Read existing settings or create new
  let settings = {};
  if (fs.existsSync(settingsPath)) {
    try {
      settings = JSON.parse(fs.readFileSync(settingsPath, 'utf8'));
    } catch (e) {
      console.warn(`[AgentTaskExecutor] Could not parse settings.json, creating new: ${e.message}`);
      settings = {};
    }
  }

  // Ensure hooks structure exists
  if (!settings.hooks) {
    settings.hooks = {};
  }
  if (!settings.hooks.PreToolUse) {
    settings.hooks.PreToolUse = [];
  }

  // Check if AskUserQuestion hook already exists
  const hasHook = settings.hooks.PreToolUse.some(
    (entry) =>
      entry.matcher === 'AskUserQuestion' ||
      (entry.hooks && entry.hooks.some((h) => h.command && h.command.includes(hookScriptName)))
  );

  if (!hasHook) {
    // Add the hook
    settings.hooks.PreToolUse.push({
      matcher: 'AskUserQuestion',
      hooks: [
        {
          type: 'command',
          command: hookScriptDst,
        },
      ],
    });

    // Write updated settings
    fs.writeFileSync(settingsPath, JSON.stringify(settings, null, 2));
    console.log(`[AgentTaskExecutor] Installed AskUserQuestion blocking hook in ${settingsPath}`);
  }

  askUserQuestionHookInstalled = true;
}

/**
 * Ensure the dangerous git blocking hook is installed in user's Claude config.
 * This blocks dangerous git commands like stash, checkout --, reset --hard, etc.
 * Modifies ~/.claude/settings.json and copies hook script to ~/.claude/hooks/
 *
 * Only used in worktree mode - Docker isolation mode has its own git-safe.sh wrapper.
 * Safe to call multiple times - only modifies config once per process.
 */
function ensureDangerousGitHook() {
  if (dangerousGitHookInstalled) {
    return; // Already installed this session
  }

  const userClaudeDir = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude');
  const hooksDir = path.join(userClaudeDir, 'hooks');
  const settingsPath = path.join(userClaudeDir, 'settings.json');
  const hookScriptName = 'block-dangerous-git.py';
  const hookScriptDst = path.join(hooksDir, hookScriptName);

  // Ensure hooks directory exists
  if (!fs.existsSync(hooksDir)) {
    fs.mkdirSync(hooksDir, { recursive: true });
  }

  // Copy hook script if not present or outdated
  const hookScriptSrc = path.join(__dirname, '..', '..', 'hooks', hookScriptName);
  if (fs.existsSync(hookScriptSrc)) {
    // Always copy to ensure latest version
    fs.copyFileSync(hookScriptSrc, hookScriptDst);
    fs.chmodSync(hookScriptDst, 0o755);
  }

  // Read existing settings or create new
  let settings = {};
  if (fs.existsSync(settingsPath)) {
    try {
      settings = JSON.parse(fs.readFileSync(settingsPath, 'utf8'));
    } catch (e) {
      console.warn(`[AgentTaskExecutor] Could not parse settings.json, creating new: ${e.message}`);
      settings = {};
    }
  }

  // Ensure hooks structure exists
  if (!settings.hooks) {
    settings.hooks = {};
  }
  if (!settings.hooks.PreToolUse) {
    settings.hooks.PreToolUse = [];
  }

  // Check if dangerous git hook already exists
  const hasHook = settings.hooks.PreToolUse.some(
    (entry) =>
      entry.matcher === 'Bash' &&
      entry.hooks &&
      entry.hooks.some((h) => h.command && h.command.includes(hookScriptName))
  );

  if (!hasHook) {
    // Add the hook - matches Bash tool to check for dangerous git commands
    settings.hooks.PreToolUse.push({
      matcher: 'Bash',
      hooks: [
        {
          type: 'command',
          command: hookScriptDst,
        },
      ],
    });

    // Write updated settings
    fs.writeFileSync(settingsPath, JSON.stringify(settings, null, 2));
    console.log(`[AgentTaskExecutor] Installed dangerous git blocking hook in ${settingsPath}`);
  }

  dangerousGitHookInstalled = true;
}

/**
 * Spawn claude-zeroshots process and stream output via message bus
 * @param {Object} agent - Agent instance
 * @param {String} context - Context to pass to Claude
 * @returns {Promise<Object>} Result object { success, output, error }
 */
async function spawnClaudeTask(agent, context) {
  const ctPath = getClaudeTasksPath();
  const cwd = agent.config.cwd || process.cwd();

  // Build zeroshot task run args.
  // CRITICAL: Default to strict schema validation to prevent cluster crashes from parse failures
  // strictSchema=true uses Claude CLI's native --json-schema enforcement (no streaming but guaranteed structure)
  // strictSchema=false uses stream-json with post-run validation (live logs but fragile)
  const desiredOutputFormat = agent.config.outputFormat || 'json';
  const strictSchema = agent.config.strictSchema !== false; // DEFAULT TO TRUE
  const runOutputFormat =
    agent.config.jsonSchema && desiredOutputFormat === 'json' && !strictSchema
      ? 'stream-json'
      : desiredOutputFormat;
  const args = ['task', 'run', '--output-format', runOutputFormat];

  // Add verification mode flag if configured
  if (agent.config.verificationMode) {
    args.push('-v');
  }

  // NOTE: maxRetries is handled by the agent wrapper's internal retry loop,
  // not passed to the CLI. See _handleTrigger() for retry logic.

  // Add JSON schema if specified in agent config.
  // If we are running stream-json for live logs (strictSchema=false), do NOT pass schema to CLI.
  if (agent.config.jsonSchema) {
    if (runOutputFormat === 'json') {
      // strictSchema=true OR no schema conflict: pass schema to CLI for native enforcement
      const schema = JSON.stringify(agent.config.jsonSchema);
      args.push('--json-schema', schema);
    } else if (!agent.quiet) {
      agent._log(
        `[Agent ${agent.id}] jsonSchema configured; running stream-json for live logs (strictSchema=false). Schema will be validated after completion.`
      );
    }
  }

  // If schema enforcement is desired but we had to run stream-json for live logs,
  // add explicit output instructions so the model still knows the required shape.
  let finalContext = context;
  if (
    agent.config.jsonSchema &&
    desiredOutputFormat === 'json' &&
    runOutputFormat === 'stream-json'
  ) {
    finalContext += `\n\n## Output Format (REQUIRED)\n\nReturn a JSON object that matches this schema exactly.\n\nSchema:\n\`\`\`json\n${JSON.stringify(
      agent.config.jsonSchema,
      null,
      2
    )}\n\`\`\`\n`;
  }

  args.push(finalContext);

  // MOCK SUPPORT: Use injected mock function if provided
  if (agent.mockSpawnFn) {
    return agent.mockSpawnFn(args, { context });
  }

  // SAFETY: Fail hard if testMode=true but no mock (should be caught in constructor)
  if (agent.testMode) {
    throw new Error(
      `AgentWrapper: testMode=true but attempting real Claude API call for agent '${agent.id}'. ` +
        `This is a bug - mock should be set in constructor.`
    );
  }

  // ISOLATION MODE: Run inside Docker container
  if (agent.isolation?.enabled) {
    return spawnClaudeTaskIsolated(agent, context);
  }

  // NON-ISOLATION MODE: Use user's existing Claude config (preserves Keychain auth)
  // AskUserQuestion blocking handled via:
  // 1. Prompt injection (see agent-context-builder) - tells agent not to ask
  // 2. PreToolUse hook (defense-in-depth) - activated by ZEROSHOT_BLOCK_ASK_USER env var
  // DO NOT override CLAUDE_CONFIG_DIR - it breaks authentication on Claude CLI 2.x
  ensureAskUserQuestionHook();

  // WORKTREE MODE: Install git safety hook (blocks dangerous git commands)
  if (agent.worktree?.enabled) {
    ensureDangerousGitHook();
  }

  // Build environment for spawn
  const spawnEnv = {
    ...process.env,
    ANTHROPIC_MODEL: agent._selectModel(),
    // Activate AskUserQuestion blocking hook (see hooks/block-ask-user-question.py)
    ZEROSHOT_BLOCK_ASK_USER: '1',
  };

  // WORKTREE MODE: Activate git safety hook via environment variable
  // The hook only activates when ZEROSHOT_WORKTREE=1 is set
  if (agent.worktree?.enabled) {
    spawnEnv.ZEROSHOT_WORKTREE = '1';
  }

  const taskId = await new Promise((resolve, reject) => {
    const proc = spawn(ctPath, args, {
      cwd,
      stdio: ['ignore', 'pipe', 'pipe'],
      env: spawnEnv,
    });
    // Track PID for resource monitoring
    agent.processPid = proc.pid;
    agent._publishLifecycle('PROCESS_SPAWNED', { pid: proc.pid });

    let stdout = '';
    let stderr = '';

    proc.stdout.on('data', (data) => {
      stdout += data.toString();
    });

    proc.stderr.on('data', (data) => {
      stderr += data.toString();
    });

    proc.on('close', (code, signal) => {
      // Handle process killed by signal (e.g., SIGTERM, SIGKILL, SIGSTOP)
      if (signal) {
        reject(new Error(`Process killed by signal ${signal}${stderr ? `: ${stderr}` : ''}`));
        return;
      }

      if (code === 0) {
        // Parse task ID from output: "✓ Task spawned: xxx-yyy-nn"
        // Format: <adjective>-<noun>-<digits> (may or may not have task- prefix)
        const match = stdout.match(/Task spawned: ((?:task-)?[a-z]+-[a-z]+-[a-z0-9]+)/);
        if (match) {
          const spawnedTaskId = match[1];
          agent.currentTaskId = spawnedTaskId; // Track for resume capability
          agent._publishLifecycle('TASK_ID_ASSIGNED', {
            pid: agent.processPid,
            taskId: spawnedTaskId,
          });

          // Start liveness monitoring
          if (agent.enableLivenessCheck) {
            agent.lastOutputTime = Date.now(); // Initialize to spawn time
            agent._startLivenessCheck();
          }

          resolve(spawnedTaskId);
        } else {
          reject(new Error(`Could not parse task ID from output: ${stdout}`));
        }
      } else {
        reject(new Error(`zeroshot task run failed with code ${code}: ${stderr}`));
      }
    });

    proc.on('error', (error) => {
      reject(error);
    });
  });

  agent._log(`📋 Agent ${agent.id}: Following zeroshot logs for ${taskId}`);

  // Wait for task to be registered in zeroshot storage (race condition fix)
  await waitForTaskReady(agent, taskId);

  // Now follow the logs and stream output
  return followClaudeTaskLogs(agent, taskId);
}

/**
 * Wait for task to be registered in ct storage
 * @param {Object} agent - Agent instance
 * @param {String} taskId - Task ID to wait for
 * @param {Number} maxRetries - Max retries (default 10)
 * @param {Number} delayMs - Delay between retries (default 200)
 * @returns {Promise<void>}
 */
async function waitForTaskReady(agent, taskId, maxRetries = 10, delayMs = 200) {
  const { exec } = require('child_process');
  const ctPath = getClaudeTasksPath();

  for (let i = 0; i < maxRetries; i++) {
    const exists = await new Promise((resolve) => {
      exec(`${ctPath} status ${taskId}`, (error, stdout) => {
        // Task exists if status doesn't return "Task not found"
        resolve(!error && !stdout.includes('Task not found'));
      });
    });

    if (exists) return;

    // Wait before retry
    await new Promise((r) => setTimeout(r, delayMs));
  }

  // Continue anyway after max retries - the task may still work
  console.warn(`⚠️ Task ${taskId} not yet visible after ${maxRetries} retries, continuing anyway`);
}

/**
 * Follow claude-zeroshots logs until completion, streaming to message bus
 * Reads log file directly for reliable streaming
 * @param {Object} agent - Agent instance
 * @param {String} taskId - Task ID to follow
 * @returns {Promise<Object>} Result object { success, output, error }
 */
function followClaudeTaskLogs(agent, taskId) {
  const fsModule = require('fs');
  const { execSync, exec } = require('child_process');
  const ctPath = getClaudeTasksPath();

  return new Promise((resolve, _reject) => {
    let output = '';
    let logFilePath = null;
    let lastSize = 0;
    let pollInterval = null;
    let statusCheckInterval = null;
    let resolved = false;

    // Get log file path from ct
    try {
      logFilePath = execSync(`${ctPath} get-log-path ${taskId}`, {
        encoding: 'utf-8',
      }).trim();
      agent._log(`📋 Agent ${agent.id}: Following ct logs for ${taskId}`);
    } catch {
      // Task might not have log file yet, wait and retry
      agent._log(`⏳ Agent ${agent.id}: Waiting for log file...`);
    }

    // Buffer for incomplete lines across polls
    let lineBuffer = '';

    // Broadcast a complete JSON line as one message
    // Lines are now prefixed with timestamps: [1733301234567]{json...}
    const broadcastLine = (line) => {
      if (!line.trim()) return;

      // Parse timestamp prefix if present: [epochMs]content
      // IMPORTANT: Trim \r from CRLF line endings before matching
      let timestamp = Date.now();
      let content = line.replace(/\r$/, '');

      const timestampMatch = content.match(/^\[(\d{13})\](.*)$/);
      if (timestampMatch) {
        timestamp = parseInt(timestampMatch[1], 10);
        content = timestampMatch[2];
      }

      // Skip known non-JSON patterns (footer, separators, metadata)
      if (
        content.startsWith('===') ||
        content.startsWith('Finished:') ||
        content.startsWith('Exit code:') ||
        (content.includes('"type":"system"') && content.includes('"subtype":"init"'))
      ) {
        return;
      }

      // Only parse lines that start with { (likely JSON)
      if (!content.trim().startsWith('{')) {
        return;
      }

      // Validate it's valid JSON before broadcasting
      try {
        JSON.parse(content);
      } catch {
        // Not valid JSON, skip silently
        return;
      }

      output += content + '\n';

      // Update liveness timestamp
      agent.lastOutputTime = Date.now();

      agent._publish({
        topic: 'AGENT_OUTPUT',
        receiver: 'broadcast',
        timestamp, // Use the actual timestamp from when output was produced
        content: {
          text: content,
          data: {
            type: 'stdout',
            line: content,
            agent: agent.id,
            role: agent.role,
            iteration: agent.iteration,
          },
        },
      });
    };

    // Process new content by splitting into complete lines
    const processNewContent = (content) => {
      // Add to buffer
      lineBuffer += content;

      // Split by newlines
      const lines = lineBuffer.split('\n');

      // Process all complete lines (all except last, which might be incomplete)
      for (let i = 0; i < lines.length - 1; i++) {
        broadcastLine(lines[i]);
      }

      // Keep last line in buffer (might be incomplete)
      lineBuffer = lines[lines.length - 1];
    };

    // Poll the log file for new content
    const pollLogFile = () => {
      // If we don't have log path yet, try to get it
      if (!logFilePath) {
        try {
          logFilePath = execSync(`${ctPath} get-log-path ${taskId}`, {
            encoding: 'utf-8',
          }).trim();
          agent._log(`📋 Agent ${agent.id}: Found log file: ${logFilePath}`);
        } catch {
          return; // Not ready yet
        }
      }

      // Check if file exists
      if (!fsModule.existsSync(logFilePath)) {
        return; // File not created yet
      }

      try {
        const stats = fsModule.statSync(logFilePath);
        const currentSize = stats.size;

        if (currentSize > lastSize) {
          // Read new content
          const fd = fsModule.openSync(logFilePath, 'r');
          const buffer = Buffer.alloc(currentSize - lastSize);
          fsModule.readSync(fd, buffer, 0, buffer.length, lastSize);
          fsModule.closeSync(fd);

          const newContent = buffer.toString('utf-8');
          // Process new content line-by-line
          processNewContent(newContent);
          lastSize = currentSize;
        }
      } catch (err) {
        // File might have been deleted or locked
        console.warn(`⚠️ Agent ${agent.id}: Error reading log: ${err.message}`);
      }
    };

    // Start polling log file (every 300ms for responsive streaming)
    pollInterval = setInterval(pollLogFile, 300);

    // Poll ct status to know when task is complete
    // Track consecutive failures for debugging stuck clusters
    let consecutiveExecFailures = 0;
    const MAX_CONSECUTIVE_FAILURES = 30; // 30 seconds of failures = log warning

    statusCheckInterval = setInterval(() => {
      exec(`${ctPath} status ${taskId}`, (error, stdout, stderr) => {
        if (resolved) return;

        // Track exec failures - if status command keeps failing, something is wrong
        if (error) {
          consecutiveExecFailures++;
          if (consecutiveExecFailures >= MAX_CONSECUTIVE_FAILURES) {
            console.error(
              `[Agent ${agent.id}] ⚠️ Status polling failed ${MAX_CONSECUTIVE_FAILURES} times consecutively! STOPPING.`
            );
            console.error(`  Command: ${ctPath} status ${taskId}`);
            console.error(`  Error: ${error.message}`);
            console.error(`  Stderr: ${stderr || 'none'}`);
            console.error(`  This may indicate zeroshot is not in PATH or task storage is corrupted.`);

            // Stop polling and resolve with failure
            if (!resolved) {
              resolved = true;
              clearInterval(pollInterval);
              clearInterval(statusCheckInterval);
              agent.currentTask = null;

              // Publish error for orchestrator/resume
              agent._publish({
                topic: 'AGENT_ERROR',
                receiver: 'broadcast',
                content: {
                  text: `Task ${taskId} polling failed after ${MAX_CONSECUTIVE_FAILURES} consecutive failures`,
                  data: {
                    taskId,
                    error: 'polling_timeout',
                    attempts: consecutiveExecFailures,
                    role: agent.role,
                    iteration: agent.iteration,
                  },
                },
              });

              resolve({
                success: false,
                output,
                error: `Status polling failed ${MAX_CONSECUTIVE_FAILURES} times - task may not exist`,
              });
            }
            return;
          }
          return; // Keep polling - might be transient
        }

        // Reset failure counter on success
        consecutiveExecFailures = 0;

        // Check for completion/failure status
        // Strip ANSI codes in case chalk outputs them (shouldn't in non-TTY, but be safe)
        // Use RegExp constructor to avoid ESLint no-control-regex false positive
        const ansiPattern = new RegExp(String.fromCharCode(27) + '\\[[0-9;]*m', 'g');
        const cleanStdout = stdout.replace(ansiPattern, '');
        // Use flexible whitespace matching in case spacing changes
        const isCompleted = /Status:\s+completed/i.test(cleanStdout);
        const isFailed = /Status:\s+failed/i.test(cleanStdout);

        if (isCompleted || isFailed) {
          const success = isCompleted;

          // Read any final content
          pollLogFile();

          // Clean up and resolve
          setTimeout(() => {
            if (resolved) return;
            resolved = true;

            clearInterval(pollInterval);
            clearInterval(statusCheckInterval);
            agent.currentTask = null;

            // Extract error context using shared helper
            const errorContext = !success
              ? extractErrorContext({ output, statusOutput: stdout, taskId })
              : null;

            resolve({
              success,
              output,
              error: errorContext,
              tokenUsage: extractTokenUsage(output),
            });
          }, 500);
        }
      });
    }, 1000);

    // Store cleanup function for kill
    // CRITICAL: Must reject promise to avoid orphaned promise that hangs forever
    agent.currentTask = {
      kill: (reason = 'Task killed') => {
        if (resolved) return;
        resolved = true;
        clearInterval(pollInterval);
        clearInterval(statusCheckInterval);
        agent._stopLivenessCheck();
        // BUGFIX: Resolve with failure instead of orphaning the promise
        // This allows the caller to handle the kill gracefully
        resolve({
          success: false,
          output,
          error: reason,
          tokenUsage: extractTokenUsage(output),
        });
      },
    };

    // REMOVED: Task timeout disabled - tasks run until completion or explicit kill
    // Tasks should run until:
    // - Completion
    // - Explicit kill
    // - External error (rate limit, API failure)
    //
    // setTimeout(() => {
    //   if (resolved) return;
    //   resolved = true;
    //
    //   clearInterval(pollInterval);
    //   clearInterval(statusCheckInterval);
    //   agent._stopLivenessCheck();
    //   agent.currentTask = null;
    //   const timeoutMinutes = Math.round(agent.timeout / 60000);
    //   reject(new Error(`Task timed out after ${timeoutMinutes} minutes`));
    // }, agent.timeout);
  });
}

/**
 * Get path to claude-zeroshots executable
 * @returns {String} Path to zeroshot command
 */
function getClaudeTasksPath() {
  // Use zeroshot command (unified CLI)
  return 'zeroshot'; // Assumes zeroshot is installed globally
}

/**
 * Spawn claude-zeroshots inside Docker container (isolation mode)
 * Runs Claude CLI inside the container for full isolation
 * @param {Object} agent - Agent instance
 * @param {String} context - Context to pass to Claude
 * @returns {Promise<Object>} Result object { success, output, error }
 */
async function spawnClaudeTaskIsolated(agent, context) {
  const { manager, clusterId } = agent.isolation;

  agent._log(`📦 Agent ${agent.id}: Running task in isolated container using zeroshot task run...`);

  // Build zeroshot task run command (same infrastructure as non-isolation mode)
  // CRITICAL: Default to strict schema validation to prevent cluster crashes from parse failures
  const desiredOutputFormat = agent.config.outputFormat || 'json';
  const strictSchema = agent.config.strictSchema !== false; // DEFAULT TO TRUE
  const runOutputFormat =
    agent.config.jsonSchema && desiredOutputFormat === 'json' && !strictSchema
      ? 'stream-json'
      : desiredOutputFormat;

  const command = ['zeroshot', 'task', 'run', '--output-format', runOutputFormat];

  // Add verification mode flag if configured
  if (agent.config.verificationMode) {
    command.push('-v');
  }

  // Add JSON schema if specified in agent config
  // If we are running stream-json for live logs (strictSchema=false), do NOT pass schema to CLI
  if (agent.config.jsonSchema) {
    if (runOutputFormat === 'json') {
      // strictSchema=true OR no schema conflict: pass schema to CLI for native enforcement
      const schema = JSON.stringify(agent.config.jsonSchema);
      command.push('--json-schema', schema);
    } else if (!agent.quiet) {
      agent._log(
        `[Agent ${agent.id}] jsonSchema configured; running stream-json for live logs (strictSchema=false). Schema will be validated after completion.`
      );
    }
  }

  // Add explicit output instructions when we run stream-json for a jsonSchema agent
  let finalContext = context;
  if (
    agent.config.jsonSchema &&
    desiredOutputFormat === 'json' &&
    runOutputFormat === 'stream-json'
  ) {
    finalContext += `\n\n## Output Format (REQUIRED)\n\nReturn a JSON object that matches this schema exactly.\n\nSchema:\n\`\`\`json\n${JSON.stringify(
      agent.config.jsonSchema,
      null,
      2
    )}\n\`\`\`\n`;
  }

  command.push(finalContext);

  // STEP 1: Spawn task and extract task ID (same as non-isolated mode)
  const taskId = await new Promise((resolve, reject) => {
    const selectedModel = agent._selectModel();
    const proc = manager.spawnInContainer(clusterId, command, {
      env: {
        ANTHROPIC_MODEL: selectedModel,
        // Activate AskUserQuestion blocking hook (see hooks/block-ask-user-question.py)
        ZEROSHOT_BLOCK_ASK_USER: '1',
      },
    });

    // Track PID for resource monitoring
    agent.processPid = proc.pid;
    agent._publishLifecycle('PROCESS_SPAWNED', { pid: proc.pid });

    let stdout = '';
    let stderr = '';

    proc.stdout.on('data', (data) => {
      stdout += data.toString();
    });

    proc.stderr.on('data', (data) => {
      stderr += data.toString();
    });

    proc.on('close', (code, signal) => {
      // Handle process killed by signal
      if (signal) {
        reject(new Error(`Process killed by signal ${signal}${stderr ? `: ${stderr}` : ''}`));
        return;
      }

      if (code === 0) {
        // Parse task ID from output: "✓ Task spawned: xxx-yyy-nn"
        const match = stdout.match(/Task spawned: ((?:task-)?[a-z]+-[a-z]+-[a-z0-9]+)/);
        if (match) {
          const spawnedTaskId = match[1];
          agent.currentTaskId = spawnedTaskId; // Track for resume capability
          agent._publishLifecycle('TASK_ID_ASSIGNED', {
            pid: agent.processPid,
            taskId: spawnedTaskId,
          });

          // Start liveness monitoring
          if (agent.enableLivenessCheck) {
            agent.lastOutputTime = Date.now(); // Initialize to spawn time
            agent._startLivenessCheck();
          }

          resolve(spawnedTaskId);
        } else {
          reject(new Error(`Could not parse task ID from output: ${stdout}`));
        }
      } else {
        reject(new Error(`zeroshot task run failed with code ${code}: ${stderr}`));
      }
    });

    proc.on('error', (error) => {
      reject(error);
    });
  });

  agent._log(`📋 Agent ${agent.id}: Following zeroshot logs for ${taskId} in container...`);

  // STEP 2: Follow the task's log file inside container (NOT the spawn stdout!)
  return followClaudeTaskLogsIsolated(agent, taskId);
}

/**
 * Follow task logs inside Docker container (isolated mode)
 * Reads task log file inside container and streams JSON lines to message bus
 * @param {Object} agent - Agent instance with isolation context
 * @param {String} taskId - Task ID to follow
 * @returns {Promise<Object>} Result object
 * @private
 */
/**
 * Follow Claude task logs in isolated container using persistent tail -f stream
 * Issue #23: Persistent log streaming instead of polling (10-20% latency reduction)
 *
 * OLD APPROACH (removed):
 * - Polled every 500ms with 2-3 docker exec calls per poll
 * - Each docker exec = ~100-200ms overhead
 * - Total: 300-400ms latency per poll cycle
 *
 * NEW APPROACH:
 * - Single persistent `tail -f` stream via spawnInContainer()
 * - Lines arrive in real-time as they're written
 * - Status checks reduced to every 2 seconds (not every poll)
 * - Result: 10-20% overall latency reduction
 */
function followClaudeTaskLogsIsolated(agent, taskId) {
  const { isolation } = agent;
  if (!isolation?.manager) {
    throw new Error('followClaudeTaskLogsIsolated: isolation manager not found');
  }

  const manager = isolation.manager;
  const clusterId = isolation.clusterId;

  return new Promise((resolve, reject) => {
    let taskExited = false;
    let fullOutput = '';
    let tailProcess = null;
    let statusCheckInterval = null;
    let lineBuffer = '';

    // Cleanup function - kill tail process and clear intervals
    const cleanup = () => {
      if (tailProcess) {
        try {
          tailProcess.kill('SIGTERM');
        } catch {
          // Ignore - process may already be dead
        }
        tailProcess = null;
      }
      if (statusCheckInterval) {
        clearInterval(statusCheckInterval);
        statusCheckInterval = null;
      }
    };

    // Broadcast line helper (same as non-isolated mode)
    const broadcastLine = (line) => {
      const timestampMatch = line.match(/^\[(\d{4}-\d{2}-\d{2}T[^\]]+)\]\s*(.*)$/);
      const timestamp = timestampMatch
        ? new Date(timestampMatch[1]).getTime()
        : Date.now();
      const content = timestampMatch ? timestampMatch[2] : line;

      agent.messageBus.publish({
        cluster_id: agent.cluster.id,
        topic: 'AGENT_OUTPUT',
        sender: agent.id,
        content: {
          data: {
            line: content,
            taskId,
            iteration: agent.iteration,
          },
        },
        timestamp,
      });

      // Update last output time for liveness tracking
      agent.lastOutputTime = Date.now();
    };

    // Process new content by splitting into complete lines
    const processNewContent = (content) => {
      lineBuffer += content;
      const lines = lineBuffer.split('\n');

      // Process all complete lines (all except last, which might be incomplete)
      for (let i = 0; i < lines.length - 1; i++) {
        if (lines[i].trim()) {
          broadcastLine(lines[i]);
        }
      }

      // Keep last line in buffer (might be incomplete)
      lineBuffer = lines[lines.length - 1];
    };

    // Get log file path from zeroshot CLI inside container
    manager
      .execInContainer(clusterId, ['sh', '-c', `zeroshot get-log-path ${taskId}`])
      .then(({ stdout, stderr, code }) => {
        if (code !== 0) {
          cleanup();
          return reject(
            new Error(
              `Failed to get log path for ${taskId} inside container: ${stderr || stdout}`
            )
          );
        }

        const logFilePath = stdout.trim();
        if (!logFilePath) {
          cleanup();
          return reject(new Error(`Empty log path returned for ${taskId}`));
        }

        agent._log(`[${agent.id}] Following isolated task logs (streaming): ${logFilePath}`);

        // Start persistent tail -f stream
        // Uses spawnInContainer() which creates a single docker exec process
        // that streams output in real-time (no polling overhead)
        tailProcess = manager.spawnInContainer(clusterId, [
          'sh',
          '-c',
          // Wait for file to exist, then tail -f from beginning
          // The -F flag handles file recreation (rotation)
          `while [ ! -f "${logFilePath}" ]; do sleep 0.1; done; tail -F -n +1 "${logFilePath}"`,
        ]);

        // Stream stdout directly - lines arrive as they're written
        tailProcess.stdout.on('data', (data) => {
          const chunk = data.toString();
          fullOutput += chunk;
          processNewContent(chunk);
        });

        // Log stderr but don't fail (tail might emit warnings)
        tailProcess.stderr.on('data', (data) => {
          const msg = data.toString().trim();
          if (msg && !msg.includes('file truncated')) {
            agent._log(`[${agent.id}] tail stderr: ${msg}`);
          }
        });

        // Handle tail process exit (shouldn't happen unless killed)
        tailProcess.on('close', (exitCode) => {
          if (!taskExited) {
            agent._log(`[${agent.id}] tail process exited with code ${exitCode}`);
          }
        });

        tailProcess.on('error', (err) => {
          agent._log(`[${agent.id}] tail process error: ${err.message}`);
        });

        // Check task status periodically (every 2 seconds - much less frequent than polling)
        // This is the only remaining docker exec - but now at 2s intervals instead of 500ms
        statusCheckInterval = setInterval(async () => {
          if (taskExited) return;

          try {
            const statusResult = await manager.execInContainer(clusterId, [
              'sh',
              '-c',
              `zeroshot status ${taskId} 2>/dev/null || echo "not_found"`,
            ]);

            const statusOutput = statusResult.stdout;
            const isSuccess = /Status:\s+completed/i.test(statusOutput);
            const isError = /Status:\s+failed/i.test(statusOutput);
            const isNotFound = statusOutput.includes('not_found');

            if (isSuccess || isError || isNotFound) {
              taskExited = true;

              // Give tail a moment to flush remaining output
              await new Promise((r) => setTimeout(r, 200));

              // Read final output to ensure we have everything
              const finalReadResult = await manager.execInContainer(clusterId, [
                'sh',
                '-c',
                `cat "${logFilePath}" 2>/dev/null || echo ""`,
              ]);

              if (finalReadResult.code === 0 && finalReadResult.stdout) {
                fullOutput = finalReadResult.stdout;

                // Process any remaining content
                const remainingLines = fullOutput.split('\n');
                for (const line of remainingLines) {
                  if (line.trim()) {
                    broadcastLine(line);
                  }
                }
              }

              cleanup();

              // Determine success status
              const success = isSuccess && !isError;

              // Extract error context using shared helper
              const errorContext = !success
                ? extractErrorContext({ output: fullOutput, taskId, isNotFound })
                : null;

              // Parse result from output
              const parsedResult = agent._parseResultOutput(fullOutput);

              resolve({
                success,
                output: fullOutput,
                taskId,
                result: parsedResult,
                error: errorContext,
                tokenUsage: extractTokenUsage(fullOutput),
              });
            }
          } catch (statusErr) {
            // Log error but continue checking (transient failures are common)
            agent._log(`[${agent.id}] Status check error (will retry): ${statusErr.message}`);
          }
        }, 2000); // Check every 2 seconds (was 500ms in polling mode)

        // Safety timeout (0 = no timeout, task runs until completion)
        if (agent.timeout > 0) {
          setTimeout(() => {
            if (!taskExited) {
              cleanup();
              reject(
                new Error(
                  `Task ${taskId} timeout after ${agent.timeout}ms (isolated mode)`
                )
              );
            }
          }, agent.timeout);
        }
      })
      .catch((err) => {
        cleanup();
        reject(err);
      });
  });
}

/**
 * Parse agent output to extract structured result data
 * GENERIC - returns whatever structured output the agent provides
 * Works with any agent schema (planner, validator, worker, etc.)
 * @param {Object} agent - Agent instance
 * @param {String} output - Raw output from agent
 * @returns {Object} Parsed result data
 */
function parseResultOutput(agent, output) {
  // Empty or error outputs = FAIL
  if (!output || output.includes('Task not found') || output.includes('Process terminated')) {
    throw new Error('Task execution failed - no output');
  }

  let parsed;
  let trimmedOutput = output.trim();

  // IMPORTANT: Output is NDJSON (one JSON object per line) from streaming log
  // Lines may have timestamp prefix: [epochMs]{json...}
  // Find the line with "type":"result" which contains the actual result
  const lines = trimmedOutput.split('\n');
  const resultLine = lines.find((line) => {
    try {
      const content = stripTimestampPrefix(line);
      if (!content.startsWith('{')) return false;
      const obj = JSON.parse(content);
      return obj.type === 'result';
    } catch {
      return false;
    }
  });

  // Use the result line if found, otherwise use last non-empty line
  // CRITICAL: Strip timestamp prefix before assigning to trimmedOutput
  if (resultLine) {
    trimmedOutput = stripTimestampPrefix(resultLine);
  } else if (lines.length > 1) {
    // Fallback: use last non-empty line (also strip timestamp)
    for (let i = lines.length - 1; i >= 0; i--) {
      const content = stripTimestampPrefix(lines[i]);
      if (content) {
        trimmedOutput = content;
        break;
      }
    }
  }

  // Strategy 1: If agent uses JSON output format, try CLI JSON structure first
  if (agent.config.outputFormat === 'json' && agent.config.jsonSchema) {
    try {
      const claudeOutput = JSON.parse(trimmedOutput);

      // Try structured_output field first (standard CLI format)
      if (claudeOutput.structured_output && typeof claudeOutput.structured_output === 'object') {
        parsed = claudeOutput.structured_output;
      }
      // Check if it's a direct object (not a primitive)
      else if (
        typeof claudeOutput === 'object' &&
        claudeOutput !== null &&
        !Array.isArray(claudeOutput)
      ) {
        // Check for result wrapper
        if (claudeOutput.result && typeof claudeOutput.result === 'object') {
          parsed = claudeOutput.result;
        }
        // IMPORTANT: Handle case where result is a string containing markdown-wrapped JSON
        // Claude CLI with --output-format json returns { result: "```json\n{...}\n```" }
        else if (claudeOutput.result && typeof claudeOutput.result === 'string') {
          const resultStr = claudeOutput.result;
          // Try extracting JSON from markdown code block
          const jsonMatch = resultStr.match(/```json\s*([\s\S]*?)```/);
          if (jsonMatch) {
            try {
              parsed = JSON.parse(jsonMatch[1].trim());
            } catch {
              // Fall through to other strategies
            }
          }
          // If no markdown block, try parsing result string directly as JSON
          if (!parsed) {
            try {
              parsed = JSON.parse(resultStr);
            } catch {
              // Fall through to other strategies
            }
          }
        }
        // Use directly if it has meaningful keys (and we haven't found a better parse)
        if (!parsed) {
          const keys = Object.keys(claudeOutput);
          if (keys.length > 0 && keys.some((k) => !['type', 'subtype', 'is_error'].includes(k))) {
            parsed = claudeOutput;
          }
        }
      }
    } catch {
      // JSON parse failed - fall through to markdown extraction
    }
  }

  // Strategy 2: Extract JSON from markdown code block (legacy or fallback)
  if (!parsed) {
    const jsonMatch = trimmedOutput.match(/```json\s*([\s\S]*?)```/);
    if (jsonMatch) {
      try {
        parsed = JSON.parse(jsonMatch[1].trim());
      } catch (e) {
        throw new Error(`JSON parse failed in markdown block: ${e.message}`);
      }
    }
  }

  // Strategy 3: Try parsing the whole output as JSON
  if (!parsed) {
    try {
      const directParse = JSON.parse(trimmedOutput);
      if (typeof directParse === 'object' && directParse !== null) {
        parsed = directParse;
      }
    } catch {
      // Not valid JSON, fall through to error
    }
  }

  // No strategy worked
  if (!parsed) {
    console.error(`\n${'='.repeat(80)}`);
    console.error(`🔴 AGENT OUTPUT MISSING REQUIRED JSON BLOCK`);
    console.error(`${'='.repeat(80)}`);
    console.error(`Agent: ${agent.id}, Role: ${agent.role}`);
    console.error(`Output (last 500 chars): ${trimmedOutput.slice(-500)}`);
    console.error(`${'='.repeat(80)}\n`);
    throw new Error(`Agent ${agent.id} output missing required JSON block`);
  }

  // If a JSON schema is configured, validate parsed output locally.
  // This preserves schema enforcement even when we run stream-json for live logs.
  // IMPORTANT: For non-validator agents we warn but do not fail the cluster.
  if (agent.config.jsonSchema) {
    const Ajv = require('ajv');
    const ajv = new Ajv({
      allErrors: true,
      strict: false,
      coerceTypes: false, // STRICT: Reject type mismatches (e.g., null instead of array)
      useDefaults: true,
      removeAdditional: true,
    });
    const validate = ajv.compile(agent.config.jsonSchema);
    const valid = validate(parsed);
    if (!valid) {
      const errorList = (validate.errors || [])
        .slice(0, 5)
        .map((e) => `${e.instancePath || e.schemaPath} ${e.message}`)
        .join('; ');
      const msg =
        `Agent ${agent.id} output failed JSON schema validation: ` +
        (errorList || 'unknown schema error');

      // Validators stay strict (they already have auto-approval fallback on crash).
      if (agent.role === 'validator') {
        throw new Error(msg);
      }

      // Non-validators: emit warning and continue with best-effort parsed data.
      console.warn(`⚠️  ${msg}`);
      agent._publish({
        topic: 'AGENT_SCHEMA_WARNING',
        receiver: 'broadcast',
        content: {
          text: msg,
          data: {
            agent: agent.id,
            role: agent.role,
            iteration: agent.iteration,
            errors: validate.errors || [],
          },
        },
      });
    }
  }

  // Return whatever the agent produced - no hardcoded field requirements
  // Template substitution will validate that required fields exist
  return parsed;
}

/**
 * Kill current task
 * @param {Object} agent - Agent instance
 */
function killTask(agent) {
  if (agent.currentTask) {
    // currentTask may be either a ChildProcess or our custom { kill } object
    if (typeof agent.currentTask.kill === 'function') {
      agent.currentTask.kill('SIGTERM');
    }
    agent.currentTask = null;
  }

  // Also kill the underlying zeroshot task if we have a task ID
  // This ensures the task process is stopped, not just our polling intervals
  if (agent.currentTaskId) {
    const { exec } = require('child_process');
    const ctPath = getClaudeTasksPath();
    exec(`${ctPath} task kill ${agent.currentTaskId}`, (error) => {
      if (error) {
        // Task may have already completed or been killed, ignore errors
        agent._log(`Note: Could not kill task ${agent.currentTaskId}: ${error.message}`);
      } else {
        agent._log(`Killed task ${agent.currentTaskId}`);
      }
    });
    agent.currentTaskId = null;
  }
}

module.exports = {
  ensureAskUserQuestionHook,
  spawnClaudeTask,
  followClaudeTaskLogs,
  waitForTaskReady,
  spawnClaudeTaskIsolated,
  getClaudeTasksPath,
  parseResultOutput,
  killTask,
};
