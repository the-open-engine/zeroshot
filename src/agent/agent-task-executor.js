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
const { exec, execSync } = require('../lib/safe-exec'); // Enforces timeouts - prevents infinite hangs
const { getProvider, parseChunkWithProvider } = require('../providers');
const { getTask } = require('../../task-lib/store.js');
const { loadSettings } = require('../../lib/settings.js');
const { resolveClaudeAuth } = require('../../lib/settings/claude-auth.js');

// Schema utilities for normalizing LLM output
const { normalizeEnumValues } = require('./schema-utils');

/**
 * Build Claude-specific environment variables for task spawning
 * Consolidates auth resolution and model mapping logic used by both isolated and non-isolated modes
 * @param {Object} modelSpec - Model specification from agent
 * @param {Object} [options] - Options
 * @param {boolean} [options.includeAuth=true] - Include auth env vars (false for isolated mode where IsolationManager handles auth)
 * @returns {Object} Environment variables to merge into spawn env
 */
function buildClaudeEnv(modelSpec, options = {}) {
  const { includeAuth = true } = options;
  const env = {};

  if (includeAuth) {
    const settings = loadSettings();
    const authEnv = resolveClaudeAuth(settings);
    Object.assign(env, authEnv);
  }

  if (modelSpec?.model) {
    env.ANTHROPIC_MODEL = modelSpec.model;
  }

  // Activate AskUserQuestion blocking hook (see hooks/block-ask-user-question.py)
  env.ZEROSHOT_BLOCK_ASK_USER = '1';

  return env;
}

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
    /^[A-Z][a-zA-Z]*\s*\|\s*(?:null|undefined)$/, // e.g., "Error | null"
  ];

  const trimmedError = error.trim();

  // Check if it's a union type like "string | number | boolean" (ReDoS-safe approach)
  const unionParts = trimmedError.split(/\s*\|\s*/);
  const isUnionType = unionParts.length > 1 && unionParts.every((p) => /^[a-z]+$/i.test(p));

  for (const pattern of typeAnnotationPatterns) {
    if (pattern.test(trimmedError) || isUnionType) {
      console.warn(
        `[agent-task-executor] WARNING: Error message looks like a TypeScript type annotation: "${error}". ` +
          `This indicates corrupted data. Replacing with generic error.`
      );
      return `Task failed with corrupted error data (original: "${error}")`;
    }
  }

  return error;
}

function safeTail(text, maxChars) {
  if (!text) return '';
  if (text.length <= maxChars) return text;
  return text.slice(-maxChars);
}

function getClaudeConfigDir() {
  return process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude');
}

function findLatestClaudeDebugFile(configDir) {
  try {
    const debugDir = path.join(configDir, 'debug');
    const latestLink = path.join(debugDir, 'latest');
    if (fs.existsSync(latestLink)) {
      const resolved = fs.realpathSync(latestLink);
      const stats = fs.statSync(resolved);
      return { path: resolved, mtimeMs: stats.mtimeMs };
    }

    const entries = fs.readdirSync(debugDir);
    let newest = null;
    for (const entry of entries) {
      const fullPath = path.join(debugDir, entry);
      const stats = fs.statSync(fullPath);
      if (!stats.isFile()) continue;
      if (!newest || stats.mtimeMs > newest.mtimeMs) {
        newest = { path: fullPath, mtimeMs: stats.mtimeMs };
      }
    }
    return newest;
  } catch (error) {
    return { error: error.message };
  }
}

function readFileTail(filePath, maxBytes) {
  try {
    const fd = fs.openSync(filePath, 'r');
    try {
      const size = fs.fstatSync(fd).size;
      const start = Math.max(0, size - maxBytes);
      const length = size - start;
      if (length <= 0) return '';
      const buffer = Buffer.alloc(length);
      fs.readSync(fd, buffer, 0, length, start);
      return buffer.toString('utf8');
    } finally {
      fs.closeSync(fd);
    }
  } catch {
    return '';
  }
}

function logNoMessagesReturned({ taskId, output, statusOutput, debug }) {
  const claudeConfigDir = getClaudeConfigDir();
  const latestDebug = findLatestClaudeDebugFile(claudeConfigDir);
  const latestDebugPath = latestDebug?.path || null;
  const latestDebugTail =
    latestDebugPath && typeof latestDebugPath === 'string'
      ? safeTail(readFileTail(latestDebugPath, 4000), 4000)
      : '';

  const payload = {
    event: 'NO_MESSAGES_RETURNED',
    timestamp: new Date().toISOString(),
    taskId,
    agentId: debug?.agentId || null,
    provider: debug?.providerName || null,
    pid: debug?.pid || null,
    cwd: debug?.cwd || null,
    worktreePath: debug?.worktreePath || null,
    isolation: debug?.isolation || false,
    clusterId: debug?.clusterId || null,
    logFilePath: debug?.logFilePath || null,
    outputLen: output ? output.length : 0,
    outputTail: safeTail(output || '', 1000),
    statusOutputLen: statusOutput ? statusOutput.length : 0,
    statusOutputTail: safeTail(statusOutput || '', 1000),
    claudeConfigDir,
    claudeDebugLatest: latestDebugPath,
    claudeDebugLatestMtimeMs: latestDebug?.mtimeMs || null,
    claudeDebugLatestTail: latestDebugTail,
  };

  console.error('[AgentTaskExecutor] Claude CLI returned no messages', payload);
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
 * @param {Object} [params.debug] - Additional debug context for logging
 * @returns {string|null} Sanitized error context or null if extraction failed
 */
function extractErrorContext({ output, statusOutput, taskId, isNotFound = false, debug }) {
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

  // KNOWN CLAUDE CODE LIMITATIONS - detect and provide actionable guidance
  const fullOutput = output || '';

  // 256KB file limit error
  if (fullOutput.includes('exceeds maximum allowed size') || fullOutput.includes('256KB')) {
    return sanitizeErrorMessage(
      `FILE TOO LARGE (Claude Code 256KB limit). ` +
        `Use offset and limit parameters when reading large files. ` +
        `Example: Read tool with offset=0, limit=1000 to read first 1000 lines.`
    );
  }

  // Streaming mode error (interactive tools in non-interactive mode)
  if (fullOutput.includes('only prompt commands are supported in streaming mode')) {
    return sanitizeErrorMessage(
      `STREAMING MODE ERROR: Agent tried to use interactive tools in streaming mode. ` +
        `This usually happens with AskUserQuestion or interactive prompts. ` +
        `Zeroshot agents must run non-interactively.`
    );
  }

  // Claude CLI transient failure: no messages returned
  if (fullOutput.includes('No messages returned')) {
    logNoMessagesReturned({ taskId, output: fullOutput, statusOutput, debug });
    return sanitizeErrorMessage(
      `Claude CLI returned no messages. This is usually transient; retry the task or resume the cluster.`
    );
  }

  // NEVER TRUNCATE OUTPUT - truncation corrupts structured JSON and causes false "crash" status
  // If output is too verbose, that's a prompt problem - fix the prompts, not the data
  const trimmedOutput = (output || '').trim();
  if (!trimmedOutput) {
    return sanitizeErrorMessage(
      'Task failed with no output (check if task was interrupted or timed out)'
    );
  }

  // Try to extract structured JSON from output first - it may contain the actual result
  // even if the task was marked as "failed" due to timeout/stale status
  try {
    const { extractJsonFromOutput } = require('./output-extraction');
    const extracted = extractJsonFromOutput(trimmedOutput);
    if (extracted && typeof extracted === 'object') {
      // If we found valid JSON, return it as the error context
      // This preserves the actual agent output for downstream processing
      return JSON.stringify(extracted);
    }
  } catch {
    // Extraction failed, fall through to error pattern matching
  }

  // Extract non-JSON lines only (JSON lines contain "is_error": true which falsely matches)
  const nonJsonLines = trimmedOutput
    .split('\n')
    .filter((line) => {
      const trimmed = line.trim();
      // Skip JSON objects and JSON-like content
      return trimmed && !trimmed.startsWith('{') && !trimmed.startsWith('"');
    })
    .join('\n');

  // Common error patterns - match against non-JSON content
  const textToSearch = nonJsonLines || trimmedOutput;
  const errorPatterns = [
    /Error:\s*(.+)/i,
    /error:\s*(.+)/i,
    /failed:\s*(.+)/i,
    /Exception:\s*(.+)/i,
    /panic:\s*(.+)/i,
  ];

  for (const pattern of errorPatterns) {
    const match = textToSearch.match(pattern);
    if (match) {
      // Don't truncate - let the full error message through
      return sanitizeErrorMessage(match[1]);
    }
  }

  // No pattern matched - return full output (no truncation)
  // If this is too long, the solution is to make agents output less, not to corrupt data
  return sanitizeErrorMessage(`Task failed. Output: ${trimmedOutput}`);
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
function extractTokenUsage(output, providerName = 'claude') {
  if (!output) return null;

  const provider = getProvider(providerName);
  const events = parseChunkWithProvider(provider, output);
  const resultEvent = events.find((event) => event.type === 'result');

  if (!resultEvent) {
    return null;
  }

  return {
    inputTokens: resultEvent.inputTokens || 0,
    outputTokens: resultEvent.outputTokens || 0,
    cacheReadInputTokens: resultEvent.cacheReadInputTokens || 0,
    cacheCreationInputTokens: resultEvent.cacheCreationInputTokens || 0,
    totalCostUsd: resultEvent.cost || null,
    durationMs: resultEvent.duration || null,
    modelUsage: resultEvent.modelUsage || null,
  };
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
  const providerName = agent._resolveProvider ? agent._resolveProvider() : 'claude';
  const modelSpec = resolveAgentModelSpec(agent);

  const ctPath = getClaudeTasksPath();
  const cwd = agent.config.cwd || process.cwd();

  // Build zeroshot task run args.
  // CRITICAL: Default to strict schema validation to prevent cluster crashes from parse failures
  // strictSchema=true uses Claude CLI's native --json-schema enforcement (no streaming but guaranteed structure)
  // strictSchema=false uses stream-json with post-run validation (live logs but fragile)
  const { desiredOutputFormat, runOutputFormat } = resolveOutputFormatConfig(agent);
  const args = buildTaskRunArgs({
    agent,
    providerName,
    modelSpec,
    runOutputFormat,
  });

  // NOTE: maxRetries is handled by the agent wrapper's internal retry loop,
  // not passed to the CLI. See _handleTrigger() for retry logic.

  maybeLogStreamJsonNotice(agent, runOutputFormat);

  // If schema enforcement is desired but we had to run stream-json for live logs,
  // add explicit output instructions so the model still knows the required shape.
  const finalContext = buildFinalContext({
    agent,
    context,
    desiredOutputFormat,
    runOutputFormat,
  });

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

  // NON-ISOLATION MODE: For Claude, use user's existing Claude config
  // AskUserQuestion blocking handled via:
  // 1. Prompt injection (see agent-context-builder)
  // 2. PreToolUse hook (defense-in-depth) - activated by ZEROSHOT_BLOCK_ASK_USER env var
  ensureProviderHooks(agent, providerName);
  const spawnEnv = buildSpawnEnv(agent, providerName, modelSpec);
  const taskId = await spawnTaskProcess({
    agent,
    ctPath,
    args,
    cwd,
    spawnEnv,
  });

  agent._log(`📋 Agent ${agent.id}: Following zeroshot logs for ${taskId}`);

  // Wait for task to be registered in zeroshot storage (race condition fix)
  await waitForTaskReady(agent, taskId);

  // CRITICAL: Poll for REAL process PID from task store
  // The watcher spawns the actual CLI and writes PID to SQLite asynchronously.
  // We must poll because the watcher runs in a forked process.
  const MAX_PID_POLLS = 30; // 3 seconds max
  const PID_POLL_DELAY = 100;
  let realPid = null;

  for (let i = 0; i < MAX_PID_POLLS; i++) {
    const taskInfo = getTask(taskId);
    if (taskInfo?.pid) {
      realPid = taskInfo.pid;
      break;
    }
    await new Promise((r) => setTimeout(r, PID_POLL_DELAY));
  }

  if (realPid) {
    agent.processPid = realPid;
    agent._publishLifecycle('PROCESS_SPAWNED', { pid: realPid });
    agent._log(`📋 Agent ${agent.id}: Process PID: ${realPid}`);
  } else {
    agent._log(`⚠️ Agent ${agent.id}: PID not available (task may use non-standard watcher)`);
  }

  // Now follow the logs and stream output
  return followClaudeTaskLogs(agent, taskId);
}

function resolveAgentModelSpec(agent) {
  return agent._resolveModelSpec ? agent._resolveModelSpec() : { model: agent._selectModel() };
}

function resolveOutputFormatConfig(agent) {
  // CRITICAL: Default to strict schema validation to prevent cluster crashes from parse failures
  // strictSchema=true uses Claude CLI's native --json-schema enforcement (no streaming but guaranteed structure)
  // strictSchema=false uses stream-json with post-run validation (live logs but fragile)
  const desiredOutputFormat = agent.config.outputFormat || 'json';
  const strictSchema = agent.config.strictSchema !== false; // DEFAULT TO TRUE
  const runOutputFormat =
    agent.config.jsonSchema && desiredOutputFormat === 'json' && !strictSchema
      ? 'stream-json'
      : desiredOutputFormat;

  return { desiredOutputFormat, strictSchema, runOutputFormat };
}

function buildTaskRunArgs({ agent, providerName, modelSpec, runOutputFormat }) {
  const args = ['task', 'run', '--output-format', runOutputFormat, '--provider', providerName];

  if (modelSpec?.model) {
    args.push('--model', modelSpec.model);
  }

  if (modelSpec?.reasoningEffort) {
    args.push('--reasoning-effort', modelSpec.reasoningEffort);
  }

  // Add verification mode flag if configured
  if (agent.config.verificationMode) {
    args.push('-v');
  }

  // Add JSON schema if specified in agent config.
  // If we are running stream-json for live logs (strictSchema=false), do NOT pass schema to CLI.
  if (agent.config.jsonSchema && runOutputFormat === 'json') {
    const schema = JSON.stringify(agent.config.jsonSchema);
    args.push('--json-schema', schema);
  }

  return args;
}

function maybeLogStreamJsonNotice(agent, runOutputFormat) {
  if (agent.config.jsonSchema && runOutputFormat !== 'json' && !agent.quiet) {
    agent._log(
      `[Agent ${agent.id}] jsonSchema configured; running stream-json for live logs (strictSchema=false). Schema will be validated after completion.`
    );
  }
}

function buildFinalContext({ agent, context, desiredOutputFormat, runOutputFormat }) {
  if (
    agent.config.jsonSchema &&
    desiredOutputFormat === 'json' &&
    runOutputFormat === 'stream-json'
  ) {
    return (
      context +
      `\n\n## Output Format (REQUIRED)\n\nReturn a JSON object that matches this schema exactly.\n\nSchema:\n\`\`\`json\n${JSON.stringify(
        agent.config.jsonSchema,
        null,
        2
      )}\n\`\`\`\n`
    );
  }

  return context;
}

function ensureProviderHooks(agent, providerName) {
  if (providerName !== 'claude') {
    return;
  }

  ensureAskUserQuestionHook();

  // WORKTREE MODE: Install git safety hook (blocks dangerous git commands)
  if (agent.worktree?.enabled) {
    ensureDangerousGitHook();
  }
}

function buildSpawnEnv(agent, providerName, modelSpec) {
  const spawnEnv = { ...process.env };

  if (providerName === 'claude') {
    Object.assign(spawnEnv, buildClaudeEnv(modelSpec));

    // WORKTREE MODE: Activate git safety hook via environment variable
    if (agent.worktree?.enabled) {
      spawnEnv.ZEROSHOT_WORKTREE = '1';
    }
  }

  return spawnEnv;
}

function parseTaskIdFromOutput(stdout) {
  const match = stdout.match(/Task spawned: ((?:task-)?[a-z]+-[a-z]+-[a-z0-9]+)/);
  return match ? match[1] : null;
}

function spawnTaskProcess({ agent, ctPath, args, cwd, spawnEnv }) {
  // Timeout for spawn phase - if CLI hangs during init (e.g., opencode 429 bug), kill it
  const SPAWN_TIMEOUT_MS = 30000; // 30 seconds to spawn task

  return new Promise((resolve, reject) => {
    const proc = spawn(ctPath, args, {
      cwd,
      stdio: ['ignore', 'pipe', 'pipe'],
      env: spawnEnv,
    });

    // NOTE: Don't emit PROCESS_SPAWNED here - proc.pid is a wrapper that exits immediately.
    // Real PID comes from task store after watcher spawns the actual CLI process.
    // PROCESS_SPAWNED is emitted in spawnClaudeTask after waitForTaskReady + PID polling.

    let stdout = '';
    let stderr = '';
    let resolved = false;

    // CRITICAL: Timeout to prevent infinite hang if provider CLI hangs
    const spawnTimeout = setTimeout(() => {
      if (resolved) return;
      resolved = true;
      proc.kill('SIGKILL');
      reject(
        new Error(
          `Spawn timeout after ${SPAWN_TIMEOUT_MS / 1000}s - provider CLI hung. ` +
            `stdout: ${stdout.slice(-500)}, stderr: ${stderr.slice(-500)}`
        )
      );
    }, SPAWN_TIMEOUT_MS);

    proc.stdout.on('data', (data) => {
      stdout += data.toString();
    });

    proc.stderr.on('data', (data) => {
      stderr += data.toString();
    });

    proc.on('close', (code, signal) => {
      clearTimeout(spawnTimeout);
      if (resolved) return;
      resolved = true;
      // Handle process killed by signal (e.g., SIGTERM, SIGKILL, SIGSTOP)
      if (signal) {
        reject(new Error(`Process killed by signal ${signal}${stderr ? `: ${stderr}` : ''}`));
        return;
      }

      if (code === 0) {
        // Parse task ID from output: "✓ Task spawned: xxx-yyy-nn"
        // Format: <adjective>-<noun>-<digits> (may or may not have task- prefix)
        const spawnedTaskId = parseTaskIdFromOutput(stdout);
        if (spawnedTaskId) {
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
      clearTimeout(spawnTimeout);
      if (resolved) return;
      resolved = true;
      reject(error);
    });
  });
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
  const ctPath = getClaudeTasksPath();

  for (let i = 0; i < maxRetries; i++) {
    let exists = false;
    try {
      const { stdout } = await exec(`${ctPath} status ${taskId}`, { timeout: 5000 });
      exists = !stdout.includes('Task not found');
    } catch {
      // Timeout or error - task not ready yet
    }

    if (exists) return;

    // Wait before retry
    await new Promise((r) => setTimeout(r, delayMs));
  }

  // FAIL FAST: Task not found after retries = unrecoverable error
  // Continuing with a non-existent task causes 30s of pointless polling then crash
  throw new Error(
    `Task ${taskId} not found after ${maxRetries} retries (${maxRetries * delayMs}ms). ` +
      `Task spawn may have failed silently. Check zeroshot task run output.`
  );
}

const MAX_STATUS_FAILURES = 30;

function createLogFollowState() {
  return {
    output: '',
    logFilePath: null,
    lastSize: 0,
    pollInterval: null,
    statusCheckInterval: null,
    resolved: false,
    lineBuffer: '',
    consecutiveExecFailures: 0,
  };
}

function lookupLogFilePath(ctPath, taskId) {
  try {
    return execSync(`${ctPath} get-log-path ${taskId}`, {
      encoding: 'utf-8',
      timeout: 5000,
    }).trim();
  } catch {
    return null;
  }
}

function parseTimestampedLine(line) {
  let timestamp = Date.now();
  let content = line.replace(/\r$/, '');

  const timestampMatch = content.match(/^\[(\d{13})\](.*)$/);
  if (timestampMatch) {
    timestamp = parseInt(timestampMatch[1], 10);
    content = timestampMatch[2];
  }

  return { timestamp, content };
}

function shouldSkipLogLine(content) {
  return (
    content.startsWith('===') ||
    content.startsWith('Finished:') ||
    content.startsWith('Exit code:') ||
    (content.includes('"type":"system"') && content.includes('"subtype":"init"'))
  );
}

function isValidJsonLine(content) {
  if (!content.trim().startsWith('{')) {
    return false;
  }

  try {
    JSON.parse(content);
    return true;
  } catch {
    return false;
  }
}

function broadcastAgentLine({ agent, providerName, state, line }) {
  if (!line.trim()) return;

  const { timestamp, content } = parseTimestampedLine(line);
  if (shouldSkipLogLine(content)) {
    return;
  }

  const isValidJson = isValidJsonLine(content);
  state.output += content + '\n';

  agent.lastOutputTime = Date.now();

  agent._publish({
    topic: 'AGENT_OUTPUT',
    receiver: 'broadcast',
    timestamp,
    content: {
      text: content,
      data: {
        type: isValidJson ? 'json' : 'text',
        line: content,
        agent: agent.id,
        role: agent.role,
        iteration: agent.iteration,
        provider: providerName,
      },
    },
  });
}

function appendContentToBuffer(state, content, onLine) {
  state.lineBuffer += content;
  const lines = state.lineBuffer.split('\n');

  for (let i = 0; i < lines.length - 1; i++) {
    onLine(lines[i]);
  }

  state.lineBuffer = lines[lines.length - 1];
}

function pollLogFileForUpdates({ agent, fsModule, ctPath, taskId, state, onNewContent }) {
  if (!state.logFilePath) {
    const logFilePath = lookupLogFilePath(ctPath, taskId);
    if (!logFilePath) {
      return;
    }
    state.logFilePath = logFilePath;
    agent._log(`📋 Agent ${agent.id}: Found log file: ${logFilePath}`);
  }

  if (!fsModule.existsSync(state.logFilePath)) {
    return;
  }

  try {
    const stats = fsModule.statSync(state.logFilePath);
    const currentSize = stats.size;

    if (currentSize > state.lastSize) {
      const fd = fsModule.openSync(state.logFilePath, 'r');
      const buffer = Buffer.alloc(currentSize - state.lastSize);
      fsModule.readSync(fd, buffer, 0, buffer.length, state.lastSize);
      fsModule.closeSync(fd);

      onNewContent(buffer.toString('utf-8'));
      state.lastSize = currentSize;
    }
  } catch (err) {
    const error = /** @type {Error} */ (err);
    console.warn(`⚠️ Agent ${agent.id}: Error reading log: ${error.message}`);
  }
}

function stripAnsiCodes(value) {
  const ansiPattern = new RegExp(String.fromCharCode(27) + '\\[[0-9;]*m', 'g');
  return value.replace(ansiPattern, '');
}

function parseStatusFlags(cleanStdout) {
  return {
    isCompleted: /Status:\s+completed/i.test(cleanStdout),
    isFailed: /Status:\s+failed/i.test(cleanStdout),
    isStale: /Status:\s+stale/i.test(cleanStdout),
  };
}

function determineStaleSuccess({ agent, output, providerName, taskId }) {
  if (!output) {
    return false;
  }

  const hasStructuredOutput = /"structured_output"\s*:/.test(output);
  const hasSuccessResult = /"subtype"\s*:\s*"success"/.test(output);
  let hasParsedOutput = false;

  try {
    const { extractJsonFromOutput } = require('./output-extraction');
    hasParsedOutput = !!extractJsonFromOutput(output, providerName);
  } catch {
    // Ignore extraction errors - fallback to other signals
  }

  const success = hasStructuredOutput || hasSuccessResult || hasParsedOutput;
  if (!agent.quiet) {
    agent._log(
      `[Agent ${agent.id}] Task ${taskId} is stale - recovered as ${success ? 'SUCCESS' : 'FAILURE'} based on output analysis`
    );
  }

  return success;
}

function finalizeLogFollow(agent, state) {
  if (state.pollInterval) {
    clearInterval(state.pollInterval);
  }
  if (state.statusCheckInterval) {
    clearInterval(state.statusCheckInterval);
  }
  agent.currentTask = null;
}

function handleStatusExecError({ agent, state, ctPath, taskId, error, stderr, resolve }) {
  if (!error) {
    return false;
  }

  state.consecutiveExecFailures++;
  if (state.consecutiveExecFailures < MAX_STATUS_FAILURES) {
    return true;
  }

  console.error(
    `[Agent ${agent.id}] ⚠️ Status polling failed ${MAX_STATUS_FAILURES} times consecutively! STOPPING.`
  );
  console.error(`  Command: ${ctPath} status ${taskId}`);
  console.error(`  Error: ${error.message}`);
  console.error(`  Stderr: ${stderr || 'none'}`);
  console.error(`  This may indicate zeroshot is not in PATH or task storage is corrupted.`);

  if (!state.resolved) {
    state.resolved = true;
    finalizeLogFollow(agent, state);

    agent._publish({
      topic: 'AGENT_ERROR',
      receiver: 'broadcast',
      content: {
        text: `Task ${taskId} polling failed after ${MAX_STATUS_FAILURES} consecutive failures`,
        data: {
          taskId,
          error: 'polling_timeout',
          attempts: state.consecutiveExecFailures,
          role: agent.role,
          iteration: agent.iteration,
        },
      },
    });

    resolve({
      success: false,
      output: state.output,
      error: `Status polling failed ${MAX_STATUS_FAILURES} times - task may not exist`,
    });
  }

  return true;
}

function handleStatusCompletion({
  agent,
  taskId,
  providerName,
  state,
  stdout,
  pollLogFile,
  resolve,
}) {
  const cleanStdout = stripAnsiCodes(stdout);
  const { isCompleted, isFailed, isStale } = parseStatusFlags(cleanStdout);

  if (!isCompleted && !isFailed && !isStale) {
    return false;
  }

  pollLogFile();

  let success = isCompleted;
  if (isStale) {
    success = determineStaleSuccess({ agent, output: state.output, providerName, taskId });
  }

  setTimeout(() => {
    if (state.resolved) return;
    state.resolved = true;

    finalizeLogFollow(agent, state);

    const errorContext = !success
      ? extractErrorContext({
          output: state.output,
          statusOutput: stdout,
          taskId,
          debug: {
            agentId: agent.id,
            providerName,
            pid: agent.processPid,
            cwd: agent.config.cwd || process.cwd(),
            worktreePath: agent.worktree?.path || null,
            isolation: !!agent.isolation?.enabled,
            logFilePath: state.logFilePath || null,
          },
        })
      : null;

    resolve({
      success,
      output: state.output,
      error: errorContext,
      tokenUsage: extractTokenUsage(state.output, providerName),
    });
  }, 500);

  return true;
}

function buildKillHandler({ agent, state, providerName, resolve }) {
  return {
    kill: (reason = 'Task killed') => {
      if (state.resolved) return;
      state.resolved = true;
      finalizeLogFollow(agent, state);
      agent._stopLivenessCheck();
      resolve({
        success: false,
        output: state.output,
        error: reason,
        tokenUsage: extractTokenUsage(state.output, providerName),
      });
    },
  };
}

function createLogFollower({ agent, taskId, fsModule, ctPath, providerName }) {
  return new Promise((resolve) => {
    const state = createLogFollowState();

    state.logFilePath = lookupLogFilePath(ctPath, taskId);
    if (state.logFilePath) {
      agent._log(`📋 Agent ${agent.id}: Following ct logs for ${taskId}`);
    } else {
      agent._log(`⏳ Agent ${agent.id}: Waiting for log file...`);
    }

    const broadcastLine = (line) => broadcastAgentLine({ agent, providerName, state, line });
    const processNewContent = (content) => appendContentToBuffer(state, content, broadcastLine);
    const pollLogFile = () =>
      pollLogFileForUpdates({
        agent,
        fsModule,
        ctPath,
        taskId,
        state,
        onNewContent: processNewContent,
      });

    state.pollInterval = setInterval(pollLogFile, 300);

    state.statusCheckInterval = setInterval(() => {
      exec(`${ctPath} status ${taskId}`, { timeout: 5000 }, (error, stdout, stderr) => {
        if (state.resolved) return;

        if (handleStatusExecError({ agent, state, ctPath, taskId, error, stderr, resolve })) {
          return;
        }

        state.consecutiveExecFailures = 0;
        handleStatusCompletion({
          agent,
          taskId,
          providerName,
          state,
          stdout,
          pollLogFile,
          resolve,
        });
      });
    }, 1000);

    agent.currentTask = buildKillHandler({ agent, state, providerName, resolve });
  });
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
  const ctPath = getClaudeTasksPath();
  const providerName = agent._resolveProvider ? agent._resolveProvider() : 'claude';

  return createLogFollower({ agent, taskId, fsModule, ctPath, providerName });
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
  const providerName = agent._resolveProvider ? agent._resolveProvider() : 'claude';
  const modelSpec = agent._resolveModelSpec
    ? agent._resolveModelSpec()
    : { model: agent._selectModel() };

  agent._log(`📦 Agent ${agent.id}: Running task in isolated container using zeroshot task run...`);

  // Build zeroshot task run command (same infrastructure as non-isolation mode)
  // CRITICAL: Default to strict schema validation to prevent cluster crashes from parse failures
  const desiredOutputFormat = agent.config.outputFormat || 'json';
  const strictSchema = agent.config.strictSchema !== false; // DEFAULT TO TRUE
  const runOutputFormat =
    agent.config.jsonSchema && desiredOutputFormat === 'json' && !strictSchema
      ? 'stream-json'
      : desiredOutputFormat;

  const command = [
    'zeroshot',
    'task',
    'run',
    '--output-format',
    runOutputFormat,
    '--provider',
    providerName,
  ];

  if (modelSpec?.model) {
    command.push('--model', modelSpec.model);
  }

  if (modelSpec?.reasoningEffort) {
    command.push('--reasoning-effort', modelSpec.reasoningEffort);
  }

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
  // Timeout for spawn phase - if CLI hangs during init (e.g., opencode 429 bug), kill it
  const SPAWN_TIMEOUT_MS = 30000; // 30 seconds to spawn task
  // Note: Auth env vars are injected by IsolationManager, we only need model mapping here
  const isolatedEnv =
    providerName === 'claude' ? buildClaudeEnv(modelSpec, { includeAuth: false }) : {};

  const taskId = await new Promise((resolve, reject) => {
    const proc = manager.spawnInContainer(clusterId, command, {
      env: isolatedEnv,
    });

    // Track PID for resource monitoring
    agent.processPid = proc.pid;
    agent._publishLifecycle('PROCESS_SPAWNED', { pid: proc.pid });

    let stdout = '';
    let stderr = '';
    let resolved = false;

    // CRITICAL: Timeout to prevent infinite hang if provider CLI hangs
    const spawnTimeout = setTimeout(() => {
      if (resolved) return;
      resolved = true;
      proc.kill('SIGKILL');
      reject(
        new Error(
          `Spawn timeout after ${SPAWN_TIMEOUT_MS / 1000}s - provider CLI hung. ` +
            `stdout: ${stdout.slice(-500)}, stderr: ${stderr.slice(-500)}`
        )
      );
    }, SPAWN_TIMEOUT_MS);

    proc.stdout.on('data', (data) => {
      stdout += data.toString();
    });

    proc.stderr.on('data', (data) => {
      stderr += data.toString();
    });

    proc.on('close', (code, signal) => {
      clearTimeout(spawnTimeout);
      if (resolved) return;
      resolved = true;
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
      clearTimeout(spawnTimeout);
      if (resolved) return;
      resolved = true;
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
function createIsolatedLogState() {
  return {
    taskExited: false,
    fullOutput: '',
    tailProcess: null,
    statusCheckInterval: null,
    lineBuffer: '',
  };
}

function buildIsolatedCleanup(state) {
  return () => {
    if (state.tailProcess) {
      try {
        state.tailProcess.kill('SIGTERM');
      } catch {
        // Ignore - process may already be dead
      }
      state.tailProcess = null;
    }
    if (state.statusCheckInterval) {
      clearInterval(state.statusCheckInterval);
      state.statusCheckInterval = null;
    }
  };
}

function broadcastIsolatedLine({ agent, providerName, taskId, line }) {
  const timestampMatch = line.match(/^\[(\d{4}-\d{2}-\d{2}T[^\]]+)\]\s*(.*)$/);
  const timestamp = timestampMatch ? new Date(timestampMatch[1]).getTime() : Date.now();
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
        provider: providerName,
      },
    },
    timestamp,
  });

  agent.lastOutputTime = Date.now();
}

function appendIsolatedContent(state, content, onLine) {
  state.lineBuffer += content;
  const lines = state.lineBuffer.split('\n');

  for (let i = 0; i < lines.length - 1; i++) {
    if (lines[i].trim()) {
      onLine(lines[i]);
    }
  }

  state.lineBuffer = lines[lines.length - 1];
}

function startIsolatedTail({ agent, manager, clusterId, logFilePath, state, onLine }) {
  state.tailProcess = manager.spawnInContainer(clusterId, [
    'sh',
    '-c',
    `while [ ! -f "${logFilePath}" ]; do sleep 0.1; done; tail -F -n +1 "${logFilePath}"`,
  ]);

  state.tailProcess.stdout.on('data', (data) => {
    const chunk = data.toString();
    state.fullOutput += chunk;
    appendIsolatedContent(state, chunk, onLine);
  });

  state.tailProcess.stderr.on('data', (data) => {
    const msg = data.toString().trim();
    if (msg && !msg.includes('file truncated')) {
      agent._log(`[${agent.id}] tail stderr: ${msg}`);
    }
  });

  state.tailProcess.on('close', (exitCode) => {
    if (!state.taskExited) {
      agent._log(`[${agent.id}] tail process exited with code ${exitCode}`);
    }
  });

  state.tailProcess.on('error', (err) => {
    agent._log(`[${agent.id}] tail process error: ${err.message}`);
  });
}

async function checkIsolatedStatus({
  agent,
  manager,
  clusterId,
  logFilePath,
  taskId,
  providerName,
  state,
  cleanup,
  resolve,
  onLine,
}) {
  if (state.taskExited) return;

  const statusResult = await manager.execInContainer(clusterId, [
    'sh',
    '-c',
    `zeroshot status ${taskId} 2>/dev/null || echo "not_found"`,
  ]);

  const statusOutput = statusResult.stdout;
  const isSuccess = /Status:\s+completed/i.test(statusOutput);
  const isError = /Status:\s+failed/i.test(statusOutput);
  const isNotFound = statusOutput.includes('not_found');

  if (!isSuccess && !isError && !isNotFound) {
    return;
  }

  state.taskExited = true;
  await new Promise((r) => setTimeout(r, 200));

  const finalReadResult = await manager.execInContainer(clusterId, [
    'sh',
    '-c',
    `cat "${logFilePath}" 2>/dev/null || echo ""`,
  ]);

  if (finalReadResult.code === 0 && finalReadResult.stdout) {
    state.fullOutput = finalReadResult.stdout;
    const remainingLines = state.fullOutput.split('\n');
    for (const line of remainingLines) {
      if (line.trim()) {
        onLine(line);
      }
    }
  }

  cleanup();

  const success = isSuccess && !isError;
  const errorContext = !success
    ? extractErrorContext({
        output: state.fullOutput,
        taskId,
        isNotFound,
        debug: {
          agentId: agent.id,
          providerName,
          pid: agent.processPid,
          cwd: agent.config.cwd || process.cwd(),
          worktreePath: agent.worktree?.path || null,
          isolation: true,
          clusterId,
          logFilePath,
        },
      })
    : null;
  const parsedResult = await agent._parseResultOutput(state.fullOutput);

  resolve({
    success,
    output: state.fullOutput,
    taskId,
    result: parsedResult,
    error: errorContext,
    tokenUsage: extractTokenUsage(state.fullOutput, providerName),
  });
}

function startIsolatedStatusChecks({
  agent,
  manager,
  clusterId,
  logFilePath,
  taskId,
  providerName,
  state,
  cleanup,
  resolve,
  onLine,
}) {
  state.statusCheckInterval = setInterval(() => {
    checkIsolatedStatus({
      agent,
      manager,
      clusterId,
      logFilePath,
      taskId,
      providerName,
      state,
      cleanup,
      resolve,
      onLine,
    }).catch((statusErr) => {
      agent._log(`[${agent.id}] Status check error (will retry): ${statusErr.message}`);
    });
  }, 2000);
}

function followClaudeTaskLogsIsolated(agent, taskId) {
  const { isolation } = agent;
  if (!isolation?.manager) {
    throw new Error('followClaudeTaskLogsIsolated: isolation manager not found');
  }

  const manager = isolation.manager;
  const clusterId = isolation.clusterId;
  const providerName = agent._resolveProvider ? agent._resolveProvider() : 'claude';

  return new Promise((resolve, reject) => {
    const state = createIsolatedLogState();
    const cleanup = buildIsolatedCleanup(state);
    const onLine = (line) => broadcastIsolatedLine({ agent, providerName, taskId, line });

    manager
      .execInContainer(clusterId, ['sh', '-c', `zeroshot get-log-path ${taskId}`])
      .then(({ stdout, stderr, code }) => {
        if (code !== 0) {
          cleanup();
          return reject(
            new Error(`Failed to get log path for ${taskId} inside container: ${stderr || stdout}`)
          );
        }

        const logFilePath = stdout.trim();
        if (!logFilePath) {
          cleanup();
          return reject(new Error(`Empty log path returned for ${taskId}`));
        }

        agent._log(`[${agent.id}] Following isolated task logs (streaming): ${logFilePath}`);

        startIsolatedTail({
          agent,
          manager,
          clusterId,
          logFilePath,
          state,
          onLine,
        });

        startIsolatedStatusChecks({
          agent,
          manager,
          clusterId,
          logFilePath,
          taskId,
          providerName,
          state,
          cleanup,
          resolve,
          onLine,
        });

        if (agent.timeout > 0) {
          setTimeout(() => {
            if (!state.taskExited) {
              cleanup();
              reject(new Error(`Task ${taskId} timeout after ${agent.timeout}ms (isolated mode)`));
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
 *
 * Uses clean extraction pipeline from output-extraction.js
 * Falls back to reformatting if extraction fails and schema is available
 *
 * @param {Object} agent - Agent instance
 * @param {String} output - Raw output from agent
 * @returns {Promise<Object>} Parsed result data
 */
async function parseResultOutput(agent, output) {
  // Empty outputs = FAIL
  if (!output || !output.trim()) {
    throw new Error('Task execution failed - no output');
  }

  const providerName = agent._resolveProvider ? agent._resolveProvider() : 'claude';
  const { extractJsonFromOutput, hasFatalStandaloneOutput } = require('./output-extraction');

  // Use clean extraction pipeline
  let parsed = extractJsonFromOutput(output, providerName);

  // If extraction failed but we have a schema, attempt reformatting
  if (!parsed && agent.config.jsonSchema) {
    const { reformatOutput } = require('./output-reformatter');

    try {
      parsed = await reformatOutput({
        rawOutput: output,
        schema: agent.config.jsonSchema,
        providerName,
        onAttempt: (attempt, lastError) => {
          if (lastError) {
            console.warn(`[Agent ${agent.id}] Reformat attempt ${attempt}: ${lastError}`);
          } else {
            console.warn(
              `[Agent ${agent.id}] JSON extraction failed, reformatting (attempt ${attempt})...`
            );
          }
        },
      });
    } catch (reformatError) {
      // Reformatting failed - fall through to error below
      console.error(`[Agent ${agent.id}] Reformatting failed: ${reformatError.message}`);
    }
  }

  if (!parsed) {
    if (hasFatalStandaloneOutput(output)) {
      throw new Error('Task execution failed - no output');
    }
    const trimmedOutput = output.trim();
    console.error(`\n${'='.repeat(80)}`);
    console.error(`🔴 AGENT OUTPUT MISSING REQUIRED JSON BLOCK`);
    console.error(`${'='.repeat(80)}`);
    console.error(`Agent: ${agent.id}, Role: ${agent.role}, Provider: ${providerName}`);
    console.error(`Output (last 500 chars): ${trimmedOutput.slice(-500)}`);
    console.error(`${'='.repeat(80)}\n`);
    throw new Error(`Agent ${agent.id} output missing required JSON block`);
  }

  // If a JSON schema is configured, validate parsed output locally.
  // This preserves schema enforcement even when we run stream-json for live logs.
  // IMPORTANT: For non-validator agents we warn but do not fail the cluster.
  if (agent.config.jsonSchema) {
    // Normalize enum values BEFORE validation (handles case mismatches, common variations)
    // This is provider-agnostic - works for Claude CLI, Gemini, Codex, etc.
    normalizeEnumValues(parsed, agent.config.jsonSchema);

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
    const ctPath = getClaudeTasksPath();
    exec(`${ctPath} task kill ${agent.currentTaskId}`, { timeout: 10000 }, (error) => {
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
