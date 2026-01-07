/**
 * ClaudeTaskRunner - Production implementation of TaskRunner
 *
 * Executes Claude tasks by spawning the `zeroshot task run` CLI command,
 * following logs, and assembling results.
 */

const { spawn, exec, execSync } = require('child_process');
const fs = require('fs');
const TaskRunner = require('./task-runner');
const { getClaudeCommand } = require('../lib/settings');

class ClaudeTaskRunner extends TaskRunner {
  /**
   * @param {Object} options
   * @param {Object} [options.messageBus] - MessageBus for streaming output
   * @param {boolean} [options.quiet] - Suppress console logging
   * @param {number} [options.timeout] - Task timeout in ms (default: 1 hour)
   * @param {Function} [options.onOutput] - Callback for output lines
   */
  constructor(options = {}) {
    super();
    this.messageBus = options.messageBus || null;
    this.quiet = options.quiet || false;
    this.timeout = options.timeout || 60 * 60 * 1000;
    this.onOutput = options.onOutput || null;
  }

  /**
   * @param {...any} args
   */
  _log(...args) {
    if (!this.quiet) {
      console.log(...args);
    }
  }

  /**
   * Execute a Claude task via zeroshot CLI
   *
   * @param {string} context - Full prompt/context
   * @param {{agentId?: string, model?: string, outputFormat?: string, jsonSchema?: any, strictSchema?: boolean, cwd?: string, isolation?: any}} options - Execution options
   * @returns {Promise<{success: boolean, output: string, error: string|null, taskId?: string}>}
   */
  async run(context, options = {}) {
    const {
      agentId = 'unknown',
      model = 'sonnet',
      outputFormat = 'stream-json',
      jsonSchema = null,
      strictSchema = false, // false = live streaming (default), true = CLI schema enforcement (no streaming)
      cwd = process.cwd(),
      isolation = null,
    } = options;

    // Isolation mode delegates to separate method
    if (isolation?.enabled) {
      return this._runIsolated(context, options);
    }

    const ctPath = 'zeroshot';

    // Build args.
    // json output does not stream; if a jsonSchema is configured we run stream-json
    // for live logs and validate/parse JSON after completion.
    // Set strictSchema=true to disable live streaming and use CLI's native schema enforcement.
    const desiredOutputFormat = outputFormat;
    const runOutputFormat =
      jsonSchema && desiredOutputFormat === 'json' && !strictSchema
        ? 'stream-json'
        : desiredOutputFormat;
    const args = ['task', 'run', '--output-format', runOutputFormat];

    // Pass schema to CLI only when using json output (strictSchema=true or no conflict)
    if (jsonSchema && runOutputFormat === 'json') {
      args.push('--json-schema', JSON.stringify(jsonSchema));
    }

    args.push(context);

    // Spawn and get task ID
    const taskId = await this._spawnAndGetTaskId(ctPath, args, cwd, model, agentId);

    this._log(`📋 [${agentId}]: Following zeroshot logs for ${taskId}`);

    // Wait for task registration
    await this._waitForTaskReady(ctPath, taskId);

    // Follow logs until completion
    return this._followLogs(ctPath, taskId, agentId);
  }

  /**
   * @param {string} ctPath
   * @param {string[]} args
   * @param {string} cwd
   * @param {string} model
   * @param {string} _agentId
   * @returns {Promise<string>}
   */
  _spawnAndGetTaskId(ctPath, args, cwd, model, _agentId) {
    return new Promise((resolve, reject) => {
      const proc = spawn(ctPath, args, {
        cwd,
        stdio: ['ignore', 'pipe', 'pipe'],
        env: {
          ...process.env,
          ANTHROPIC_MODEL: model,
        },
      });

      let stdout = '';
      let stderr = '';

      proc.stdout.on('data', (data) => {
        stdout += data.toString();
      });

      proc.stderr.on('data', (data) => {
        stderr += data.toString();
      });

      proc.on('close', (code) => {
        if (code === 0) {
          const match = stdout.match(/Task spawned: ((?:task-)?[a-z]+-[a-z]+-[a-z0-9]+)/);
          if (match) {
            resolve(match[1]);
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
  }

  /**
   * @param {string} ctPath
   * @param {string} taskId
   * @param {number} maxRetries
   * @param {number} delayMs
   * @returns {Promise<void>}
   */
  async _waitForTaskReady(ctPath, taskId, maxRetries = 10, delayMs = 200) {
    for (let i = 0; i < maxRetries; i++) {
      const exists = await new Promise((resolve) => {
        exec(`${ctPath} status ${taskId}`, (error, stdout) => {
          resolve(!error && !stdout.includes('Task not found'));
        });
      });

      if (exists) return;
      await new Promise((r) => setTimeout(r, delayMs));
    }
    console.warn(
      `⚠️ Task ${taskId} not yet visible after ${maxRetries} retries, continuing anyway`
    );
  }

  /**
   * @param {string} ctPath
   * @param {string} taskId
   * @param {string} agentId
   * @returns {Promise<{success: boolean, output: string, error: string|null, taskId: string}>}
   */
  _followLogs(ctPath, taskId, agentId) {
    return new Promise((resolve, reject) => {
      let output = '';
      /** @type {string|null} */
      let logFilePath = null;
      let lastSize = 0;
      /** @type {NodeJS.Timeout|null} */
      let pollInterval = null;
      /** @type {NodeJS.Timeout|null} */
      let statusCheckInterval = null;
      let resolved = false;
      let lineBuffer = '';

      // Get log file path
      try {
        logFilePath = execSync(`${ctPath} get-log-path ${taskId}`, {
          encoding: 'utf-8',
        }).trim();
      } catch {
        this._log(`⏳ [${agentId}]: Waiting for log file...`);
      }

      /**
       * @param {string} line
       */
      const broadcastLine = (line) => {
        if (!line.trim()) return;

        let content = line;
        const timestampMatch = line.match(/^\[(\d{13})\](.*)$/);
        if (timestampMatch) {
          content = timestampMatch[2];
        }

        // Skip non-JSON patterns
        if (
          content.startsWith('===') ||
          content.startsWith('Finished:') ||
          content.startsWith('Exit code:') ||
          (content.includes('"type":"system"') && content.includes('"subtype":"init"'))
        ) {
          return;
        }

        if (!content.trim().startsWith('{')) return;

        try {
          JSON.parse(content);
        } catch {
          return;
        }

        output += content + '\n';

        // Callback for output streaming
        if (this.onOutput) {
          this.onOutput(content, agentId);
        }
      };

      /**
       * @param {string} content
       */
      const processNewContent = (content) => {
        lineBuffer += content;
        const lines = lineBuffer.split('\n');

        for (let i = 0; i < lines.length - 1; i++) {
          broadcastLine(lines[i]);
        }
        lineBuffer = lines[lines.length - 1];
      };

      const pollLogFile = () => {
        if (!logFilePath) {
          try {
            logFilePath = execSync(`${ctPath} get-log-path ${taskId}`, {
              encoding: 'utf-8',
            }).trim();
          } catch {
            return;
          }
        }

        if (!fs.existsSync(logFilePath)) return;

        try {
          const stats = fs.statSync(logFilePath);
          const currentSize = stats.size;

          if (currentSize > lastSize) {
            const fd = fs.openSync(logFilePath, 'r');
            const buffer = Buffer.alloc(currentSize - lastSize);
            fs.readSync(fd, buffer, 0, buffer.length, lastSize);
            fs.closeSync(fd);

            processNewContent(buffer.toString('utf-8'));
            lastSize = currentSize;
          }
        } catch (err) {
          const error = /** @type {Error} */ (err);
          console.warn(`⚠️ [${agentId}]: Error reading log: ${error.message}`);
        }
      };

      pollInterval = setInterval(pollLogFile, 300);

      /**
       * @param {boolean} success
       * @param {string} stdout
       * @returns {string|null}
       */
      const extractErrorContext = (success, stdout) => {
        if (success) return null;

        // Try to extract error from status output first
        const statusErrorMatch = stdout.match(/Error:\s*(.+)/);
        if (statusErrorMatch) {
          return statusErrorMatch[1].trim();
        }

        // Fall back to last 500 chars of output
        const lastOutput = output.slice(-500).trim();
        if (!lastOutput) {
          return 'Task failed with no output';
        }

        const errorPatterns = [
          /Error:\s*(.+)/i,
          /error:\s*(.+)/i,
          /failed:\s*(.+)/i,
          /Exception:\s*(.+)/i,
        ];

        for (const pattern of errorPatterns) {
          const match = lastOutput.match(pattern);
          if (match) {
            return match[1].slice(0, 200);
          }
        }

        return `Task failed. Last output: ${lastOutput.slice(-200)}`;
      };

      statusCheckInterval = setInterval(() => {
        exec(`${ctPath} status ${taskId}`, (error, stdout) => {
          if (resolved) return;

          if (
            !error &&
            (stdout.includes('Status:     completed') || stdout.includes('Status:     failed'))
          ) {
            const success = stdout.includes('Status:     completed');

            pollLogFile();

            setTimeout(() => {
              if (resolved) return;
              resolved = true;

              if (pollInterval) clearInterval(pollInterval);
              if (statusCheckInterval) clearInterval(statusCheckInterval);

              const errorContext = extractErrorContext(success, stdout);

              resolve({
                success,
                output,
                error: errorContext,
                taskId,
              });
            }, 500);
          }
        });
      }, 1000);

      // Timeout
      setTimeout(() => {
        if (resolved) return;
        resolved = true;

        clearInterval(pollInterval);
        clearInterval(statusCheckInterval);

        const timeoutMinutes = Math.round(this.timeout / 60000);
        reject(new Error(`Task timed out after ${timeoutMinutes} minutes`));
      }, this.timeout);
    });
  }

  /**
   * Run task in isolated Docker container
   * @param {string} context
   * @param {{agentId?: string, model?: string, outputFormat?: string, jsonSchema?: any, strictSchema?: boolean, isolation?: any}} options
   * @returns {Promise<{success: boolean, output: string, error: string|null}>}
   */
  _runIsolated(context, options) {
    const {
      agentId = 'unknown',
      model = 'sonnet',
      outputFormat = 'stream-json',
      jsonSchema = null,
      strictSchema = false,
      isolation,
    } = options;
    const { manager, clusterId } = isolation;

    this._log(`📦 [${agentId}]: Running task in isolated container...`);

    // Determine output format: stream-json for live logs unless strictSchema=true
    const desiredOutputFormat = outputFormat;
    const runOutputFormat =
      jsonSchema && desiredOutputFormat === 'json' && !strictSchema
        ? 'stream-json'
        : desiredOutputFormat;

    // Get configured Claude command (supports custom commands like 'ccr code')
    const { command: claudeCmd, args: claudeExtraArgs } = getClaudeCommand();

    const command = [
      claudeCmd,
      ...claudeExtraArgs,
      '--print',
      '--dangerously-skip-permissions',
      '--output-format',
      runOutputFormat,
    ];

    if (runOutputFormat === 'stream-json') {
      command.push('--verbose');
      command.push('--include-partial-messages');
    }

    // Pass schema to CLI only when using json output (strictSchema=true or no conflict)
    if (jsonSchema && runOutputFormat === 'json') {
      command.push('--json-schema', JSON.stringify(jsonSchema));
    }

    if (model) {
      command.push('--model', model);
    }

    command.push(context);

    return new Promise((resolve, reject) => {
      let output = '';
      let resolved = false;

      const proc = manager.spawnInContainer(clusterId, command, {
        env: { ANTHROPIC_MODEL: model },
      });

      proc.stdout.on('data', (/** @type {Buffer} */ data) => {
        const chunk = data.toString();
        output += chunk;

        if (this.onOutput) {
          this.onOutput(chunk, agentId);
        }
      });

      proc.stderr.on('data', (/** @type {Buffer} */ data) => {
        const chunk = data.toString();
        if (!this.quiet) {
          console.error(`[${agentId}] stderr:`, chunk);
        }
      });

      proc.on('close', (/** @type {number|null} */ code) => {
        if (resolved) return;
        resolved = true;

        resolve({
          success: code === 0,
          output,
          error: code === 0 ? null : `Container exited with code ${code}`,
        });
      });

      proc.on('error', (/** @type {Error} */ error) => {
        if (resolved) return;
        resolved = true;
        reject(error);
      });

      setTimeout(() => {
        if (resolved) return;
        resolved = true;

        try {
          proc.kill('SIGKILL');
        } catch {
          // Ignore - process may already be dead
        }

        const timeoutMinutes = Math.round(this.timeout / 60000);
        reject(new Error(`Isolated task timed out after ${timeoutMinutes} minutes`));
      }, this.timeout);
    });
  }
}

module.exports = ClaudeTaskRunner;
