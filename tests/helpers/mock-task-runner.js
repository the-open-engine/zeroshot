/**
 * MockTaskRunner - Test implementation of TaskRunner interface
 *
 * Provides a fluent API for configuring behavior per agent and tracking invocations.
 * Useful for testing coordination logic without spawning actual Claude processes.
 */
const assert = require('node:assert');
const TaskRunner = require('../../src/task-runner.js');
const Ajv = require('ajv');

class MockTaskRunner extends TaskRunner {
  constructor() {
    super();
    this.behaviors = new Map();
    this.calls = [];
    this.streamEvents = new Map();
    this.ajv = new Ajv({ allErrors: true, strict: false });
  }

  when(agentId) {
    const self = this;

    return {
      withModel(model) {
        const existingBehavior = self.behaviors.get(agentId) || {
          type: 'success',
          output: '{}',
        };
        existingBehavior.expectedModel = model;
        self.behaviors.set(agentId, existingBehavior);

        return {
          returns(output) {
            const behavior = self.behaviors.get(agentId);
            behavior.output = typeof output === 'string' ? output : JSON.stringify(output);

            if (!behavior.ms && behavior.expectedModel) {
              behavior.type = 'delay';
              behavior.ms = self._getModelDefaultDelay(behavior.expectedModel);
            } else {
              behavior.type = 'success';
            }

            self.behaviors.set(agentId, behavior);
            return self;
          },

          fails(error) {
            const behavior = self.behaviors.get(agentId);
            behavior.type = 'error';
            behavior.error = error instanceof Error ? error.message : error;
            self.behaviors.set(agentId, behavior);
            return self;
          },

          delays(ms, output) {
            const behavior = self.behaviors.get(agentId);
            behavior.type = 'delay';
            behavior.ms = ms;
            behavior.output = typeof output === 'string' ? output : JSON.stringify(output);
            self.behaviors.set(agentId, behavior);
            return self;
          },

          calls(fn) {
            const behavior = self.behaviors.get(agentId);
            behavior.type = 'function';
            behavior.fn = fn;
            self.behaviors.set(agentId, behavior);
            return self;
          },

          streams(events, delayMs = 50) {
            const behavior = self.behaviors.get(agentId);
            behavior.type = 'streaming';
            behavior.events = events;
            behavior.delayMs = delayMs;

            return {
              thenReturns(output) {
                behavior.output = typeof output === 'string' ? output : JSON.stringify(output);
                self.behaviors.set(agentId, behavior);
                return self;
              },
            };
          },
        };
      },

      withOutputFormat(format) {
        if (!['text', 'json', 'stream-json'].includes(format)) {
          throw new Error(
            `Invalid output format: ${format}. Must be 'text', 'json', or 'stream-json'`
          );
        }
        const behavior = self.behaviors.get(agentId) || {
          type: 'success',
          output: '{}',
        };
        behavior.outputFormat = format;
        self.behaviors.set(agentId, behavior);
        return this;
      },

      withJsonSchema(schema) {
        if (!schema || typeof schema !== 'object') {
          throw new Error('JSON schema must be an object');
        }
        const behavior = self.behaviors.get(agentId) || {
          type: 'success',
          output: '{}',
        };
        behavior.jsonSchema = schema;
        self.behaviors.set(agentId, behavior);
        return this;
      },

      returns(output) {
        const behavior = self.behaviors.get(agentId) || {
          type: 'success',
          output: '{}',
        };
        behavior.type = 'success';
        behavior.output = typeof output === 'string' ? output : JSON.stringify(output);
        self.behaviors.set(agentId, behavior);
        return self;
      },

      fails(error) {
        const behavior = self.behaviors.get(agentId) || {
          type: 'error',
          error: '',
        };
        behavior.type = 'error';
        behavior.error = error instanceof Error ? error.message : error;
        self.behaviors.set(agentId, behavior);
        return self;
      },

      delays(ms, output) {
        const behavior = self.behaviors.get(agentId) || {
          type: 'delay',
          ms: 0,
          output: '{}',
        };
        behavior.type = 'delay';
        behavior.ms = ms;
        behavior.output = typeof output === 'string' ? output : JSON.stringify(output);
        self.behaviors.set(agentId, behavior);
        return self;
      },

      calls(fn) {
        const behavior = self.behaviors.get(agentId) || { type: 'function' };
        behavior.type = 'function';
        behavior.fn = fn;
        self.behaviors.set(agentId, behavior);
        return self;
      },

      streams(events, delayMs = 50) {
        const behavior = self.behaviors.get(agentId) || { type: 'streaming' };
        behavior.type = 'streaming';
        behavior.events = events;
        behavior.delayMs = delayMs;

        return {
          thenReturns(output) {
            behavior.output = typeof output === 'string' ? output : JSON.stringify(output);
            self.behaviors.set(agentId, behavior);
            return self;
          },
        };
      },

      failsWithRateLimit(retryAfter) {
        const behavior = self.behaviors.get(agentId) || { type: 'rate_limit' };
        behavior.type = 'rate_limit';
        behavior.retryAfter = retryAfter;
        self.behaviors.set(agentId, behavior);
        return self;
      },

      failsWithAuth(message = 'Authentication failed') {
        const behavior = self.behaviors.get(agentId) || { type: 'auth_error' };
        behavior.type = 'auth_error';
        behavior.message = message;
        self.behaviors.set(agentId, behavior);
        return self;
      },

      failsWithMalformed(partial = '{"incomplete":') {
        const behavior = self.behaviors.get(agentId) || { type: 'malformed' };
        behavior.type = 'malformed';
        behavior.partial = partial;
        self.behaviors.set(agentId, behavior);
        return self;
      },

      failsWithTimeout() {
        const behavior = self.behaviors.get(agentId) || { type: 'timeout' };
        behavior.type = 'timeout';
        self.behaviors.set(agentId, behavior);
        return self;
      },

      failsWithNetworkError() {
        const behavior = self.behaviors.get(agentId) || {
          type: 'network_error',
        };
        behavior.type = 'network_error';
        self.behaviors.set(agentId, behavior);
        return self;
      },

      failsOnCall(callNumber, errorType) {
        const existingBehavior = self.behaviors.get(agentId);
        self.behaviors.set(agentId, {
          type: 'conditional',
          callNumber,
          errorType,
          fallbackBehavior: existingBehavior || {
            type: 'success',
            output: '{}',
          },
        });
        return {
          thenReturns(output) {
            const behavior = self.behaviors.get(agentId);
            behavior.fallbackBehavior = {
              type: 'success',
              output: typeof output === 'string' ? output : JSON.stringify(output),
            };
            return self;
          },
        };
      },
    };
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

  async run(context, options) {
    const { agentId, model } = options;

    const callCount = this.calls.filter((c) => c.agentId === agentId).length + 1;

    const callRecord = {
      agentId,
      context,
      options,
      timestamp: Date.now(),
      callNumber: callCount,
      streamEvents: [],
    };
    this.calls.push(callRecord);

    let behavior = this.behaviors.get(agentId) || {
      type: 'success',
      output: '{}',
    };

    // Validate model if expectedModel is set
    if (behavior.expectedModel && model !== behavior.expectedModel) {
      throw new Error(
        `Expected agent "${agentId}" to be called with model "${behavior.expectedModel}", but was called with "${model}"`
      );
    }

    // Handle conditional behavior (fail on specific call)
    if (behavior.type === 'conditional') {
      if (callCount === behavior.callNumber) {
        behavior = this._getErrorBehavior(behavior.errorType);
      } else {
        behavior = behavior.fallbackBehavior;
      }
    }

    let result;
    switch (behavior.type) {
      case 'success':
        result = {
          success: true,
          output: behavior.output,
          error: null,
        };
        break;

      case 'error':
        result = {
          success: false,
          output: '',
          error: behavior.error,
        };
        break;

      case 'delay':
        await new Promise((resolve) => setTimeout(resolve, behavior.ms));
        result = {
          success: true,
          output: behavior.output,
          error: null,
        };
        break;

      case 'function':
        result = behavior.fn(context, options);
        break;

      case 'streaming':
        for (const event of behavior.events) {
          callRecord.streamEvents.push(event);

          if (options.onOutput) {
            const eventJson = JSON.stringify(event);
            options.onOutput(eventJson, agentId);
          }

          if (behavior.delayMs > 0) {
            await new Promise((resolve) => setTimeout(resolve, behavior.delayMs));
          }
        }

        if (!this.streamEvents.has(agentId)) {
          this.streamEvents.set(agentId, []);
        }
        this.streamEvents.get(agentId).push([...behavior.events]);

        result = {
          success: true,
          output: behavior.output || '{}',
          error: null,
        };
        break;

      case 'rate_limit':
        result = {
          success: false,
          output: '',
          error: `Rate limit exceeded. Retry after ${behavior.retryAfter} seconds.`,
          errorType: 'RATE_LIMIT',
          retryAfter: behavior.retryAfter,
        };
        break;

      case 'auth_error':
        result = {
          success: false,
          output: '',
          error: behavior.message,
          errorType: 'AUTH_ERROR',
        };
        break;

      case 'malformed':
        result = {
          success: false,
          output: behavior.partial,
          error: 'Malformed response from API',
          errorType: 'MALFORMED_RESPONSE',
        };
        break;

      case 'timeout':
        result = {
          success: false,
          output: '',
          error: 'Request timed out',
          errorType: 'TIMEOUT',
        };
        break;

      case 'network_error':
        result = {
          success: false,
          output: '',
          error: 'Network connection failed',
          errorType: 'NETWORK_ERROR',
        };
        break;

      default:
        result = {
          success: true,
          output: '{}',
          error: null,
        };
    }

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
