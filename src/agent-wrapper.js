// @ts-nocheck
/**
 * AgentWrapper - Manages agent lifecycle and claude-zeroshots execution
 *
 * Provides:
 * - Agent state machine (idle -> evaluating -> building context -> executing -> idle)
 * - Trigger evaluation via LogicEngine
 * - Context building from ledger
 * - claude-zeroshots spawning and monitoring
 * - Hook execution (onStart, onComplete, onError)
 */

const LogicEngine = require('./logic-engine');
const { validateAgentConfig } = require('./agent/agent-config');
const { loadSettings, validateModelAgainstMax } = require('../lib/settings');
const { buildContext } = require('./agent/agent-context-builder');
const { findMatchingTrigger, evaluateTrigger } = require('./agent/agent-trigger-evaluator');
const { executeHook } = require('./agent/agent-hook-executor');
const {
  spawnClaudeTask,
  followClaudeTaskLogs,
  waitForTaskReady,
  spawnClaudeTaskIsolated,
  getClaudeTasksPath,
  parseResultOutput,
  killTask,
} = require('./agent/agent-task-executor');
const {
  start: lifecycleStart,
  stop: lifecycleStop,
  handleMessage: lifecycleHandleMessage,
  executeTriggerAction: lifecycleExecuteTriggerAction,
  executeTask: lifecycleExecuteTask,
  startLivenessCheck: lifecycleStartLivenessCheck,
  stopLivenessCheck: lifecycleStopLivenessCheck,
} = require('./agent/agent-lifecycle');

class AgentWrapper {
  /**
   * @param {any} config - Agent configuration
   * @param {any} messageBus - Message bus instance
   * @param {any} cluster - Cluster instance
   * @param {any} options - Options
   */
  constructor(config, messageBus, cluster, options = {}) {
    // Validate and normalize configuration
    const normalizedConfig = validateAgentConfig(config, options);

    this.id = normalizedConfig.id;
    this.role = normalizedConfig.role;
    this.modelConfig = normalizedConfig.modelConfig;
    this.config = normalizedConfig;
    this.messageBus = messageBus;
    this.cluster = cluster;
    this.logicEngine = new LogicEngine(messageBus, cluster);

    this.state = 'idle';
    this.iteration = 0;
    this.maxIterations = normalizedConfig.maxIterations;
    this.timeout = normalizedConfig.timeout;
    /** @type {any} */
    this.currentTask = null;
    /** @type {string | null} */
    this.currentTaskId = null; // Track spawned task ID for resume capability
    /** @type {number | null} */
    this.processPid = null; // Track process PID for resource monitoring
    this.running = false;
    /** @type {Function | null} */
    this.unsubscribe = null;
    /** @type {number | null} */
    this.lastTaskEndTime = null; // Track when last task completed (for context filtering)

    // LIVENESS DETECTION - Track output freshness to detect stuck agents
    /** @type {number | null} */
    this.lastOutputTime = null; // Timestamp of last output received
    /** @type {NodeJS.Timeout | null} */
    this.livenessCheckInterval = null; // Interval for health checks
    this.staleDuration = normalizedConfig.staleDuration;
    this.enableLivenessCheck = normalizedConfig.enableLivenessCheck;

    // MOCK SUPPORT - Inject mock spawn function for testing
    // When set, _spawnClaudeTask uses this instead of real ct CLI
    this.mockSpawnFn = options.mockSpawnFn || null;
    this.testMode = options.testMode || false;
    this.quiet = options.quiet || false;

    // ISOLATION SUPPORT - Run tasks inside Docker container
    this.isolation = options.isolation || null;
  }

  /**
   * Log message (respects quiet mode)
   * @private
   */
  _log(...args) {
    if (!this.quiet) {
      console.log(...args);
    }
  }

  /**
   * Publish a message to the message bus, always including sender_model
   * @private
   */
  _publish(message) {
    this.messageBus.publish({
      ...message,
      cluster_id: this.cluster.id,
      sender: this.id,
      sender_model: this._selectModel(),
    });
  }

  /**
   * Publish agent lifecycle event to message bus (visible in zeroshot logs)
   * @private
   */
  _publishLifecycle(event, details = {}) {
    this._publish({
      topic: 'AGENT_LIFECYCLE',
      receiver: 'system',
      content: {
        text: `${this.id}: ${event}`,
        data: {
          event,
          agent: this.id,
          role: this.role,
          state: this.state,
          model: this._selectModel(),
          ...details,
        },
      },
    });
  }

  /**
   * Select model based on current iteration and agent config
   * Enforces maxModel ceiling from settings
   * @returns {string} Model name ('sonnet', 'opus', 'haiku')
   * @private
   */
  _selectModel() {
    const settings = loadSettings();
    const maxModel = settings.maxModel || 'sonnet';

    let requestedModel = null;

    // Get requested model from config
    if (this.modelConfig.type === 'static') {
      requestedModel = this.modelConfig.model;
    } else if (this.modelConfig.type === 'rules') {
      // Dynamic rules: evaluate based on iteration
      for (const rule of this.modelConfig.rules) {
        if (this._matchesIterationRange(rule.iterations)) {
          requestedModel = rule.model;
          break;
        }
      }

      // No match for rules: fail fast (config error)
      if (!requestedModel) {
        throw new Error(
          `Agent ${this.id}: No model rule matched iteration ${this.iteration}. ` +
            `Add a catch-all rule like { "iterations": "all", "model": "sonnet" }`
        );
      }
    }

    // If no model specified (neither static nor rules), use maxModel as default
    if (!requestedModel) {
      return maxModel;
    }

    // Enforce ceiling - will throw if requestedModel > maxModel
    return validateModelAgainstMax(requestedModel, maxModel);
  }

  /**
   * Check if current iteration matches the range pattern
   * @param {string} pattern - e.g., "1", "1-3", "5+", "all"
   * @returns {boolean}
   * @private
   */
  _matchesIterationRange(pattern) {
    if (pattern === 'all') return true;

    const current = this.iteration;

    // Exact match: "3"
    if (/^\d+$/.test(pattern)) {
      return current === parseInt(pattern);
    }

    // Range: "1-3"
    const rangeMatch = pattern.match(/^(\d+)-(\d+)$/);
    if (rangeMatch) {
      const [, start, end] = rangeMatch;
      return current >= parseInt(start) && current <= parseInt(end);
    }

    // Open-ended: "5+"
    const openMatch = pattern.match(/^(\d+)\+$/);
    if (openMatch) {
      const [, start] = openMatch;
      return current >= parseInt(start);
    }

    throw new Error(
      `Agent ${this.id}: Invalid iteration pattern '${pattern}'. ` +
        `Valid formats: "1", "1-3", "5+", "all"`
    );
  }

  /**
   * Select prompt based on current iteration and agent config
   * @returns {string|null} System prompt string, or null if no prompt configured
   * @private
   */
  _selectPrompt() {
    const promptConfig = this.config.promptConfig;

    // No prompt configured
    if (!promptConfig) {
      return null;
    }

    // Backward compatibility: static prompt
    if (promptConfig.type === 'static') {
      return promptConfig.system;
    }

    // Dynamic rules: evaluate based on iteration
    for (const rule of promptConfig.rules) {
      if (this._matchesIterationRange(rule.match)) {
        return rule.system;
      }
    }

    // No match: fail fast
    throw new Error(
      `Agent ${this.id}: No prompt rule matched iteration ${this.iteration}. ` +
        `Add a catch-all rule like { "match": "all", "system": "..." }`
    );
  }

  /**
   * Start the agent (begin listening for triggers)
   */
  start() {
    return lifecycleStart(this);
  }

  /**
   * Stop the agent
   */
  stop() {
    return lifecycleStop(this);
  }

  /**
   * Handle incoming message
   * @private
   */
  _handleMessage(message) {
    return lifecycleHandleMessage(this, message);
  }

  /**
   * Find trigger matching the message topic
   * @private
   */
  _findMatchingTrigger(message) {
    return findMatchingTrigger({
      triggers: this.config.triggers,
      message,
    });
  }

  /**
   * Evaluate trigger logic
   * @private
   */
  _evaluateTrigger(trigger, message) {
    const agent = {
      id: this.id,
      role: this.role,
      iteration: this.iteration,
      cluster_id: this.cluster.id,
    };

    return evaluateTrigger({
      trigger,
      message,
      agent,
      logicEngine: this.logicEngine,
    });
  }

  /**
   * Execute trigger action
   * @private
   */
  _executeTriggerAction(trigger, message) {
    return lifecycleExecuteTriggerAction(this, trigger, message);
  }

  /**
   * Execute claude-zeroshots with built context
   * Retries disabled by default. Set agent config `maxRetries` to enable (e.g., 3).
   * @private
   */
  _executeTask(triggeringMessage) {
    return lifecycleExecuteTask(this, triggeringMessage);
  }

  /**
   * Build context from ledger based on contextStrategy
   * @private
   */
  _buildContext(triggeringMessage) {
    return buildContext({
      id: this.id,
      role: this.role,
      iteration: this.iteration,
      config: this.config,
      messageBus: this.messageBus,
      cluster: this.cluster,
      lastTaskEndTime: this.lastTaskEndTime,
      triggeringMessage,
      selectedPrompt: this._selectPrompt(),
    });
  }

  /**
   * Spawn claude-zeroshots process and stream output via message bus
   * @private
   */
  _spawnClaudeTask(context) {
    return spawnClaudeTask(this, context);
  }

  /**
   * Wait for task to be registered in ct storage
   * @private
   */
  _waitForTaskReady(taskId, maxRetries = 10, delayMs = 200) {
    return waitForTaskReady(this, taskId, maxRetries, delayMs);
  }

  /**
   * Follow claude-zeroshots logs until completion, streaming to message bus
   * Reads log file directly for reliable streaming
   * @private
   */
  _followClaudeTaskLogs(taskId) {
    return followClaudeTaskLogs(this, taskId);
  }

  /**
   * Get path to claude-zeroshots executable
   * @private
   */
  _getClaudeTasksPath() {
    return getClaudeTasksPath();
  }

  /**
   * Spawn claude-zeroshots inside Docker container (isolation mode)
   * Runs Claude CLI inside the container for full isolation
   * @private
   */
  _spawnClaudeTaskIsolated(context) {
    return spawnClaudeTaskIsolated(this, context);
  }

  /**
   * Kill current task
   * @private
   */
  _killTask() {
    return killTask(this);
  }

  /**
   * Execute a hook
   * THROWS on failure - no silent errors
   * @private
   */
  _executeHook(hookName, context) {
    const hook = this.config.hooks?.[hookName];
    return executeHook({
      hook,
      agent: this,
      message: context.triggeringMessage,
      result: context.result,
      messageBus: this.messageBus,
      cluster: this.cluster,
      orchestrator: this.orchestrator,
    });
  }

  /**
   * Parse agent output to extract structured result data
   * GENERIC - returns whatever structured output the agent provides
   * Works with any agent schema (planner, validator, worker, etc.)
   * @private
   */
  _parseResultOutput(output) {
    return parseResultOutput(this, output);
  }

  /**
   * Resume agent task with context from previous failure
   * Called by Orchestrator.resume() to continue where we left off
   * @param {String} resumeContext - Context describing what to resume
   */
  async resume(resumeContext) {
    if (!this.running) {
      throw new Error(`Agent ${this.id} is not running. Start it first.`);
    }

    if (this.state !== 'idle') {
      throw new Error(`Agent ${this.id} is busy (state: ${this.state}). Wait for current task.`);
    }

    this._log(`[${this.id}] Resuming task...`);

    // Create a synthetic triggering message for resume
    const triggeringMessage = {
      cluster_id: this.cluster.id,
      topic: 'AGENT_RESUME',
      sender: 'system',
      content: {
        text: resumeContext,
      },
    };

    // Execute the task with resume context
    await this._executeTask(triggeringMessage);
  }

  /**
   * Get current agent state
   */
  getState() {
    return {
      id: this.id,
      role: this.role,
      model: this._selectModel(),
      state: this.state,
      iteration: this.iteration,
      maxIterations: this.maxIterations,
      currentTask: this.currentTask ? true : false,
      currentTaskId: this.currentTaskId,
      pid: this.processPid,
    };
  }

  /**
   * Start monitoring agent output liveness
   * Detects when agent produces no output for configured staleDuration
   * @private
   */
  _startLivenessCheck() {
    return lifecycleStartLivenessCheck(this);
  }

  /**
   * Stop liveness monitoring
   * @private
   */
  _stopLivenessCheck() {
    return lifecycleStopLivenessCheck(this);
  }
}

module.exports = AgentWrapper;
