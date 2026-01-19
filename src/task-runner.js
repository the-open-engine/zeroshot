/**
 * TaskRunner - Strategy Pattern interface for executing provider tasks
 *
 * Implementations must provide a `run()` method that executes a task
 * with the given context and options. Different runners can implement various
 * execution strategies (CLI, mock responses, etc).
 */
class TaskRunner {
  /**
   * Execute a provider task with the given context and options
   *
   * @param {string} _context - Full prompt/context for the provider to process
   * @param {Object} _options - Execution options
   * @param {string} _options.agentId - Identifier for this agent/task
   * @param {string} _options.model - Model to use (provider-specific model id)
   * @param {string} [_options.provider] - Provider to use (claude|codex|gemini)
   * @param {object} [_options.modelSpec] - Resolved model spec (level/model/reasoningEffort)
   * @param {string} [_options.outputFormat] - Output format ('text', 'json', 'stream-json')
   * @param {Object} [_options.jsonSchema] - JSON schema for structured output validation
   * @param {string} [_options.cwd] - Working directory for task execution
   * @param {boolean} [_options.isolation] - Whether to run in isolated container
   *
   * @returns {Promise<{success: boolean, output: string, error: string|null, taskId?: string}>} Result object with success status, output, error message, and optional taskId
   */
  run(_context, _options) {
    throw new Error('TaskRunner.run() not implemented');
  }
}

module.exports = TaskRunner;
