/**
 * TEST: MockTaskRunner Error Scenario Simulation
 *
 * Verifies that MockTaskRunner can simulate various error conditions
 * from Claude CLI (rate limits, auth failures, timeouts, network errors, malformed responses).
 * This allows testing retry logic and error handling without hitting real APIs.
 */

const assert = require('assert');
const MockTaskRunner = require('./helpers/mock-task-runner');

let mockRunner;

describe('MockTaskRunner Error Scenarios', () => {
  beforeEach(() => {
    mockRunner = new MockTaskRunner();
  });

  defineRateLimitErrorTests();
  defineAuthenticationErrorTests();
  defineMalformedResponseTests();
  defineTimeoutErrorTests();
  defineNetworkErrorTests();
  defineIntermittentFailureTests();
  defineErrorStructureValidationTests();
  defineCallTrackingTests();
});

function defineRateLimitErrorTests() {
  describe('Rate Limit Errors', () => {
    it('should simulate rate limit error with retry-after', async () => {
      mockRunner.when('agent').failsWithRateLimit(30);

      const result = await mockRunner.run('Test context', { agentId: 'agent' });

      assert.strictEqual(result.success, false);
      assert.strictEqual(result.error, 'Rate limit exceeded. Retry after 30 seconds.');
      assert.strictEqual(result.errorType, 'RATE_LIMIT');
      assert.strictEqual(result.retryAfter, 30);
    });

    it('should allow testing retry logic after rate limit', async () => {
      // First call fails with rate limit, subsequent calls succeed
      mockRunner.when('agent').failsOnCall(1, 'rate_limit').thenReturns({ ok: true });

      const result1 = await mockRunner.run('Test', { agentId: 'agent' });
      assert.strictEqual(result1.success, false);
      assert.strictEqual(result1.errorType, 'RATE_LIMIT');

      const result2 = await mockRunner.run('Test', { agentId: 'agent' });
      assert.strictEqual(result2.success, true);
      assert.strictEqual(result2.output, JSON.stringify({ ok: true }));
    });
  });
}

function defineAuthenticationErrorTests() {
  describe('Authentication Errors', () => {
    it('should simulate authentication failure with default message', async () => {
      mockRunner.when('agent').failsWithAuth();

      const result = await mockRunner.run('Test context', { agentId: 'agent' });

      assert.strictEqual(result.success, false);
      assert.strictEqual(result.error, 'Authentication failed');
      assert.strictEqual(result.errorType, 'AUTH_ERROR');
    });

    it('should simulate authentication failure with custom message', async () => {
      mockRunner.when('agent').failsWithAuth('API key expired');

      const result = await mockRunner.run('Test context', { agentId: 'agent' });

      assert.strictEqual(result.success, false);
      assert.strictEqual(result.error, 'API key expired');
      assert.strictEqual(result.errorType, 'AUTH_ERROR');
    });
  });
}

function defineMalformedResponseTests() {
  describe('Malformed Response Errors', () => {
    it('should simulate malformed response with default partial', async () => {
      mockRunner.when('agent').failsWithMalformed();

      const result = await mockRunner.run('Test context', { agentId: 'agent' });

      assert.strictEqual(result.success, false);
      assert.strictEqual(result.error, 'Malformed response from API');
      assert.strictEqual(result.errorType, 'MALFORMED_RESPONSE');
      assert.strictEqual(result.output, '{"incomplete":');
    });

    it('should simulate malformed response with custom partial', async () => {
      mockRunner.when('agent').failsWithMalformed('{"data": [1, 2,');

      const result = await mockRunner.run('Test context', { agentId: 'agent' });

      assert.strictEqual(result.success, false);
      assert.strictEqual(result.errorType, 'MALFORMED_RESPONSE');
      assert.strictEqual(result.output, '{"data": [1, 2,');
    });
  });
}

function defineTimeoutErrorTests() {
  describe('Timeout Errors', () => {
    it('should simulate request timeout', async () => {
      mockRunner.when('agent').failsWithTimeout();

      const result = await mockRunner.run('Test context', { agentId: 'agent' });

      assert.strictEqual(result.success, false);
      assert.strictEqual(result.error, 'Request timed out');
      assert.strictEqual(result.errorType, 'TIMEOUT');
    });
  });
}

function defineNetworkErrorTests() {
  describe('Network Errors', () => {
    it('should simulate network connection failure', async () => {
      mockRunner.when('agent').failsWithNetworkError();

      const result = await mockRunner.run('Test context', { agentId: 'agent' });

      assert.strictEqual(result.success, false);
      assert.strictEqual(result.error, 'Network connection failed');
      assert.strictEqual(result.errorType, 'NETWORK_ERROR');
    });
  });
}

function defineIntermittentFailureTests() {
  describe('Intermittent Failures', () => {
    it('should fail on specific call number then succeed', async () => {
      mockRunner.when('agent').failsOnCall(2, 'timeout').thenReturns({ success: true });

      // First call succeeds (default behavior before error)
      const result1 = await mockRunner.run('Test', { agentId: 'agent' });
      assert.strictEqual(result1.success, true);

      // Second call times out
      const result2 = await mockRunner.run('Test', { agentId: 'agent' });
      assert.strictEqual(result2.success, false);
      assert.strictEqual(result2.errorType, 'TIMEOUT');

      // Third call succeeds (fallback behavior)
      const result3 = await mockRunner.run('Test', { agentId: 'agent' });
      assert.strictEqual(result3.success, true);
      assert.strictEqual(result3.output, JSON.stringify({ success: true }));
    });

    it('should handle multiple intermittent failures', async () => {
      // Fail on calls 1 and 3, succeed otherwise
      mockRunner.when('agent1').failsOnCall(1, 'network').thenReturns({ ok: true });
      mockRunner.when('agent2').failsOnCall(2, 'auth').thenReturns({ ok: true });

      // Agent1: call 1 fails
      const a1r1 = await mockRunner.run('Test', { agentId: 'agent1' });
      assert.strictEqual(a1r1.success, false);
      assert.strictEqual(a1r1.errorType, 'NETWORK_ERROR');

      // Agent1: call 2 succeeds
      const a1r2 = await mockRunner.run('Test', { agentId: 'agent1' });
      assert.strictEqual(a1r2.success, true);

      // Agent2: call 1 succeeds
      const a2r1 = await mockRunner.run('Test', { agentId: 'agent2' });
      assert.strictEqual(a2r1.success, true);

      // Agent2: call 2 fails
      const a2r2 = await mockRunner.run('Test', { agentId: 'agent2' });
      assert.strictEqual(a2r2.success, false);
      assert.strictEqual(a2r2.errorType, 'AUTH_ERROR');

      // Agent2: call 3 succeeds
      const a2r3 = await mockRunner.run('Test', { agentId: 'agent2' });
      assert.strictEqual(a2r3.success, true);
    });
  });
}

function defineErrorStructureValidationTests() {
  describe('Error Structure Validation', () => {
    it('should return consistent error structure for all error types', async () => {
      const errorTypes = [
        {
          method: 'failsWithRateLimit',
          args: [60],
          expectedType: 'RATE_LIMIT',
        },
        { method: 'failsWithAuth', args: [], expectedType: 'AUTH_ERROR' },
        {
          method: 'failsWithMalformed',
          args: [],
          expectedType: 'MALFORMED_RESPONSE',
        },
        { method: 'failsWithTimeout', args: [], expectedType: 'TIMEOUT' },
        {
          method: 'failsWithNetworkError',
          args: [],
          expectedType: 'NETWORK_ERROR',
        },
      ];

      for (const { method, args, expectedType } of errorTypes) {
        mockRunner.reset();
        mockRunner.when('agent')[method](...args);

        const result = await mockRunner.run('Test', { agentId: 'agent' });

        assert.strictEqual(result.success, false, `${method} should set success to false`);
        assert.strictEqual(
          typeof result.error,
          'string',
          `${method} should return error as string`
        );
        assert.strictEqual(
          result.errorType,
          expectedType,
          `${method} should set correct errorType`
        );
      }
    });
  });
}

function defineCallTrackingTests() {
  describe('Call Tracking with Errors', () => {
    it('should track call numbers correctly with intermittent failures', async () => {
      mockRunner.when('agent').failsOnCall(2, 'timeout').thenReturns({ ok: true });

      await mockRunner.run('Test 1', { agentId: 'agent' });
      await mockRunner.run('Test 2', { agentId: 'agent' });
      await mockRunner.run('Test 3', { agentId: 'agent' });

      const calls = mockRunner.getCalls('agent');
      assert.strictEqual(calls.length, 3);
      assert.strictEqual(calls[0].callNumber, 1);
      assert.strictEqual(calls[1].callNumber, 2);
      assert.strictEqual(calls[2].callNumber, 3);
    });
  });
}
