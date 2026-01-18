/**
 * MockTaskRunner - Test implementation of TaskRunner interface
 *
 * Provides a fluent API for configuring behavior per agent and tracking invocations.
 * Useful for testing coordination logic without spawning actual Claude processes.
 */
const assert = require('node:assert');
const TaskRunner = require('../../src/task-runner.js');
const Ajv = require('ajv');

class BehaviorBuilder {
  constructor(runner, agentId) {
    this.runner = runner;
    this.agentId = agentId;
  }

  _getBehavior(defaultBehavior) {
    const existingBehavior = this.runner.behaviors.get(this.agentId);
    if (existingBehavior) {
      return existingBehavior;
    }

    const behavior = { ...defaultBehavior };
    this.runner.behaviors.set(this.agentId, behavior);
    return behavior;
  }

  withModel(model) {
    const behavior = this._getBehavior({ type: 'success', output: '{}' });
    behavior.expectedModel = model;
    this.runner.behaviors.set(this.agentId, behavior);
    return this;
  }

  withOutputFormat(format) {
    if (!['text', 'json', 'stream-json'].includes(format)) {
      throw new Error(`Invalid output format: ${format}. Must be 'text', 'json', or 'stream-json'`);
    }
    const behavior = this._getBehavior({ type: 'success', output: '{}' });
    behavior.outputFormat = format;
    this.runner.behaviors.set(this.agentId, behavior);
    return this;
  }

  withJsonSchema(schema) {
    if (!schema || typeof schema !== 'object') {
      throw new Error('JSON schema must be an object');
    }
    const behavior = this._getBehavior({ type: 'success', output: '{}' });
    behavior.jsonSchema = schema;
    this.runner.behaviors.set(this.agentId, behavior);
    return this;
  }

  returns(output) {
    const behavior = this._getBehavior({ type: 'success', output: '{}' });
    behavior.output = typeof output === 'string' ? output : JSON.stringify(output);

    if (!behavior.ms && behavior.expectedModel) {
      behavior.type = 'delay';
      behavior.ms = this.runner._getModelDefaultDelay(behavior.expectedModel);
    } else {
      behavior.type = 'success';
    }

    this.runner.behaviors.set(this.agentId, behavior);
    return this.runner;
  }

  fails(error) {
    const behavior = this._getBehavior({ type: 'error', error: '' });
    behavior.type = 'error';
    behavior.error = error instanceof Error ? error.message : error;
    this.runner.behaviors.set(this.agentId, behavior);
    return this.runner;
  }

  delays(ms, output) {
    const behavior = this._getBehavior({ type: 'delay', ms: 0, output: '{}' });
    behavior.type = 'delay';
    behavior.ms = ms;
    behavior.output = typeof output === 'string' ? output : JSON.stringify(output);
    this.runner.behaviors.set(this.agentId, behavior);
    return this.runner;
  }

  calls(fn) {
    const behavior = this._getBehavior({ type: 'function' });
    behavior.type = 'function';
    behavior.fn = fn;
    this.runner.behaviors.set(this.agentId, behavior);
    return this.runner;
  }

  streams(events, delayMs = 50) {
    const behavior = this._getBehavior({ type: 'streaming' });
    behavior.type = 'streaming';
    behavior.events = events;
    behavior.delayMs = delayMs;

    return {
      thenReturns: (output) => {
        behavior.output = typeof output === 'string' ? output : JSON.stringify(output);
        this.runner.behaviors.set(this.agentId, behavior);
        return this.runner;
      },
    };
  }

  failsWithRateLimit(retryAfter) {
    const behavior = this._getBehavior({ type: 'rate_limit' });
    behavior.type = 'rate_limit';
    behavior.retryAfter = retryAfter;
    this.runner.behaviors.set(this.agentId, behavior);
    return this.runner;
  }

  failsWithAuth(message = 'Authentication failed') {
    const behavior = this._getBehavior({ type: 'auth_error' });
    behavior.type = 'auth_error';
    behavior.message = message;
    this.runner.behaviors.set(this.agentId, behavior);
    return this.runner;
  }

  failsWithMalformed(partial = '{"incomplete":') {
    const behavior = this._getBehavior({ type: 'malformed' });
    behavior.type = 'malformed';
    behavior.partial = partial;
    this.runner.behaviors.set(this.agentId, behavior);
    return this.runner;
  }

  failsWithTimeout() {
    const behavior = this._getBehavior({ type: 'timeout' });
    behavior.type = 'timeout';
    this.runner.behaviors.set(this.agentId, behavior);
    return this.runner;
  }

  failsWithNetworkError() {
    const behavior = this._getBehavior({ type: 'network_error' });
    behavior.type = 'network_error';
    this.runner.behaviors.set(this.agentId, behavior);
    return this.runner;
  }

  failsOnCall(callNumber, errorType) {
    const existingBehavior = this.runner.behaviors.get(this.agentId);
    this.runner.behaviors.set(this.agentId, {
      type: 'conditional',
      callNumber,
      errorType,
      fallbackBehavior: existingBehavior || {
        type: 'success',
        output: '{}',
      },
    });

    return {
      thenReturns: (output) => {
        const behavior = this.runner.behaviors.get(this.agentId);
        behavior.fallbackBehavior = {
          type: 'success',
          output: typeof output === 'string' ? output : JSON.stringify(output),
        };
        return this.runner;
      },
    };
  }
}

class MockTaskRunner extends TaskRunner {
  constructor() {
    super();
    this.behaviors = new Map();
    this.calls = [];
    this.streamEvents = new Map();
    this.ajv = new Ajv({ allErrors: true, strict: false });
  }

  when(agentId) {
    return new BehaviorBuilder(this, agentId);
  }

  _getModelDefaultDelay(model) {
    const modelDefaults = {
      opus: 2000,
      sonnet: 1000,
      haiku: 500,
    };
    return modelDefaults[model] || 1000;
  }

  _getErrorBehavior(errorType) {
    switch (errorType) {
      case 'timeout':
        return { type: 'timeout' };
      case 'rate_limit':
        return { type: 'rate_limit', retryAfter: 60 };
      case 'network':
        return { type: 'network_error' };
      case 'auth':
        return { type: 'auth_error', message: 'Authentication failed' };
      case 'malformed':
        return { type: 'malformed', partial: '{"incomplete":' };
      default:
        return { type: 'error', error: 'Unknown error' };
    }
  }

  _validateOutput(output, behavior) {
    // No schema = no validation
    if (!behavior.jsonSchema) {
      return { valid: true };
    }

    // Only validate if format is json or stream-json
    const format = behavior.outputFormat || 'text';
    if (format !== 'json' && format !== 'stream-json') {
      return { valid: true };
    }

    // Parse output
    let parsed;
    try {
      parsed = JSON.parse(output);
    } catch (err) {
      return {
        valid: false,
        errors: [`Output is not valid JSON: ${err.message}`],
      };
    }

    // Validate against schema
    const validate = this.ajv.compile(behavior.jsonSchema);
    const valid = validate(parsed);

    if (!valid) {
      const errors = validate.errors.map((e) => {
        // ajv v6 uses dataPath, v7+ uses instancePath
        const path = e.instancePath || e.dataPath || '/';
        return `${path} ${e.message}`;
      });
      return { valid: false, errors };
    }

    return { valid: true };
  }

  _sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms));
  }

  _resolveModel(options) {
    if (options.model) {
      return options.model;
    }

    switch (options.modelLevel) {
      case 'level1':
        return 'haiku';
      case 'level2':
        return 'sonnet';
      case 'level3':
        return 'opus';
      default:
        return undefined;
    }
  }

  _recordCall(agentId, context, options, resolvedModel) {
    const callCount = this.calls.filter((c) => c.agentId === agentId).length + 1;
    const callRecord = {
      agentId,
      context,
      options: { ...options, model: resolvedModel },
      timestamp: Date.now(),
      callNumber: callCount,
      streamEvents: [],
    };
    this.calls.push(callRecord);
    return { callCount, callRecord };
  }

  _resolveBehavior(agentId, resolvedModel, callCount) {
    let behavior = this.behaviors.get(agentId) || {
      type: 'success',
      output: '{}',
    };

    if (behavior.expectedModel && resolvedModel !== behavior.expectedModel) {
      throw new Error(
        `Expected agent "${agentId}" to be called with model "${behavior.expectedModel}", but was called with "${resolvedModel}"`
      );
    }

    if (behavior.type === 'conditional') {
      if (callCount === behavior.callNumber) {
        return this._getErrorBehavior(behavior.errorType);
      }
      return behavior.fallbackBehavior;
    }

    return behavior;
  }

  async _handleDelayBehavior(behavior) {
    await this._sleep(behavior.ms);
    return {
      success: true,
      output: behavior.output,
      error: null,
    };
  }

  async _handleStreamingBehavior(behavior, options, callRecord, agentId) {
    for (const event of behavior.events) {
      callRecord.streamEvents.push(event);

      if (options.onOutput) {
        const eventJson = JSON.stringify(event);
        options.onOutput(eventJson, agentId);
      }

      if (behavior.delayMs > 0) {
        await this._sleep(behavior.delayMs);
      }
    }

    if (!this.streamEvents.has(agentId)) {
      this.streamEvents.set(agentId, []);
    }
    this.streamEvents.get(agentId).push([...behavior.events]);

    return {
      success: true,
      output: behavior.output || '{}',
      error: null,
    };
  }

  _executeBehavior(behavior, context, options, callRecord, agentId) {
    switch (behavior.type) {
      case 'success':
        return {
          success: true,
          output: behavior.output,
          error: null,
        };
      case 'error':
        return {
          success: false,
          output: '',
          error: behavior.error,
        };
      case 'delay':
        return this._handleDelayBehavior(behavior);
      case 'function':
        return behavior.fn(context, options);
      case 'streaming':
        return this._handleStreamingBehavior(behavior, options, callRecord, agentId);
      case 'rate_limit':
        return {
          success: false,
          output: '',
          error: `Rate limit exceeded. Retry after ${behavior.retryAfter} seconds.`,
          errorType: 'RATE_LIMIT',
          retryAfter: behavior.retryAfter,
        };
      case 'auth_error':
        return {
          success: false,
          output: '',
          error: behavior.message,
          errorType: 'AUTH_ERROR',
        };
      case 'malformed':
        return {
          success: false,
          output: behavior.partial,
          error: 'Malformed response from API',
          errorType: 'MALFORMED_RESPONSE',
        };
      case 'timeout':
        return {
          success: false,
          output: '',
          error: 'Request timed out',
          errorType: 'TIMEOUT',
        };
      case 'network_error':
        return {
          success: false,
          output: '',
          error: 'Network connection failed',
          errorType: 'NETWORK_ERROR',
        };
      default:
        return {
          success: true,
          output: '{}',
          error: null,
        };
    }
  }

  async run(context, options) {
    const { agentId } = options;
    const resolvedModel = this._resolveModel(options);
    const { callCount, callRecord } = this._recordCall(agentId, context, options, resolvedModel);
    const behavior = this._resolveBehavior(agentId, resolvedModel, callCount);
    const result = await this._executeBehavior(behavior, context, options, callRecord, agentId);

    // Validate output if schema is configured and result is success
    if (result.success && result.output) {
      const validation = this._validateOutput(result.output, behavior);
      if (!validation.valid) {
        return {
          success: false,
          output: result.output,
          error: `Output validation failed: ${validation.errors.join(', ')}`,
        };
      }
    }

    return result;
  }

  assertCalled(agentId, times = 1) {
    const callCount = this.calls.filter((c) => c.agentId === agentId).length;
    assert.strictEqual(
      callCount,
      times,
      'Expected agent "' +
        agentId +
        '" to be called ' +
        times +
        ' times, but was called ' +
        callCount +
        ' times'
    );
  }

  assertCalledWith(agentId, matcherFn) {
    const calls = this.calls.filter((c) => c.agentId === agentId);
    const matching = calls.filter(matcherFn);

    assert(
      matching.length > 0,
      'Expected agent "' + agentId + '" to be called with matching context, but no calls matched'
    );
  }

  assertCalledWithModel(agentId, expectedModel) {
    const calls = this.calls.filter((c) => c.agentId === agentId);

    assert(calls.length > 0, `Expected agent "${agentId}" to be called, but it was never called`);

    const lastCall = calls[calls.length - 1];
    assert.strictEqual(
      lastCall.options.model,
      expectedModel,
      `Expected agent "${agentId}" to be called with model "${expectedModel}", but was called with "${lastCall.options.model}"`
    );
  }

  assertCalledWithOutputFormat(agentId, expectedFormat) {
    const calls = this.getCalls(agentId);
    assert(
      calls.length > 0,
      'Expected agent "' + agentId + '" to be called, but it was never called'
    );

    const matching = calls.filter((c) => c.options.outputFormat === expectedFormat);
    assert(
      matching.length > 0,
      'Expected agent "' +
        agentId +
        '" to be called with outputFormat "' +
        expectedFormat +
        '", but found: ' +
        calls.map((c) => c.options.outputFormat || 'undefined').join(', ')
    );
  }

  assertCalledWithJsonSchema(agentId, expectedSchema) {
    const calls = this.getCalls(agentId);
    assert(
      calls.length > 0,
      'Expected agent "' + agentId + '" to be called, but it was never called'
    );

    const matching = calls.filter((c) => {
      if (!c.options.jsonSchema) return false;
      return JSON.stringify(c.options.jsonSchema) === JSON.stringify(expectedSchema);
    });

    assert(
      matching.length > 0,
      'Expected agent "' +
        agentId +
        '" to be called with specific JSON schema, but none matched. Found schemas: ' +
        calls.map((c) => JSON.stringify(c.options.jsonSchema || null)).join(' | ')
    );
  }

  assertContextIncludes(agentId, expectedSubstring) {
    const calls = this.getCalls(agentId);
    assert(
      calls.length > 0,
      'Expected agent "' + agentId + '" to be called, but it was never called'
    );

    const matching = calls.filter((c) => c.context.includes(expectedSubstring));
    assert(
      matching.length > 0,
      'Expected agent "' +
        agentId +
        '" context to include "' +
        expectedSubstring +
        '", but no calls matched'
    );
  }

  assertContextExcludes(agentId, unexpectedSubstring) {
    const calls = this.getCalls(agentId);
    assert(
      calls.length > 0,
      'Expected agent "' + agentId + '" to be called, but it was never called'
    );

    const matching = calls.filter((c) => c.context.includes(unexpectedSubstring));
    assert(
      matching.length === 0,
      'Expected agent "' +
        agentId +
        '" context to exclude "' +
        unexpectedSubstring +
        '", but found it in ' +
        matching.length +
        ' call(s)'
    );
  }

  assertCalledWithOptions(agentId, expectedOptions) {
    const calls = this.getCalls(agentId);
    assert(
      calls.length > 0,
      'Expected agent "' + agentId + '" to be called, but it was never called'
    );

    const matching = calls.filter((c) => {
      return Object.keys(expectedOptions).every((key) => {
        const expected = expectedOptions[key];
        const actual = c.options[key];

        if (typeof expected === 'object' && expected !== null) {
          return JSON.stringify(actual) === JSON.stringify(expected);
        }

        return actual === expected;
      });
    });

    assert(
      matching.length > 0,
      'Expected agent "' +
        agentId +
        '" to be called with options ' +
        JSON.stringify(expectedOptions) +
        ', but no calls matched. Found: ' +
        calls.map((c) => JSON.stringify(c.options)).join(' | ')
    );
  }

  getCalls(agentId) {
    return this.calls.filter((c) => c.agentId === agentId);
  }

  getStreamEvents(agentId, callIndex = 0) {
    const calls = this.getCalls(agentId);
    if (callIndex >= calls.length) {
      return [];
    }
    return calls[callIndex].streamEvents || [];
  }

  assertStreamedEvents(agentId, expectedEvents, callIndex = 0) {
    const actualEvents = this.getStreamEvents(agentId, callIndex);

    assert.strictEqual(
      actualEvents.length,
      expectedEvents.length,
      'Expected ' + expectedEvents.length + ' stream events, but got ' + actualEvents.length
    );

    for (let i = 0; i < expectedEvents.length; i++) {
      const expected = expectedEvents[i];
      const actual = actualEvents[i];

      if (typeof expected === 'object' && expected !== null) {
        for (const key in expected) {
          assert(
            key in actual,
            'Expected event ' + i + ' to have key "' + key + '", but it was missing'
          );

          if (typeof expected[key] !== 'object' || expected[key] === null) {
            assert.strictEqual(
              actual[key],
              expected[key],
              'Expected event ' +
                i +
                '.' +
                key +
                ' to be "' +
                expected[key] +
                '", but got "' +
                actual[key] +
                '"'
            );
          }
        }
      } else {
        assert.deepStrictEqual(actual, expected, 'Event ' + i + ' did not match expected event');
      }
    }
  }

  reset() {
    this.behaviors.clear();
    this.calls = [];
    this.streamEvents.clear();
  }
}

module.exports = MockTaskRunner;
