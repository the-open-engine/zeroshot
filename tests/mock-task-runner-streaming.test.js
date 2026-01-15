/**
 * Tests for MockTaskRunner streaming support
 *
 * Verifies that the MockTaskRunner correctly handles streaming events,
 * emits them with proper delays, and provides assertions for verification.
 */
const assert = require('assert');
const MockTaskRunner = require('./helpers/mock-task-runner');

let runner;

describe('MockTaskRunner Streaming', () => {
  beforeEach(() => {
    runner = new MockTaskRunner();
  });

  defineBasicStreamingTests();
  defineEventDelayTests();
  defineOnOutputCallbackTests();
  defineMixedBehaviorTests();
  defineAssertStreamedEventsTests();
  defineGetStreamEventsTests();
  defineRealWorldUseCaseTests();
  defineResetTests();
});

function defineBasicStreamingTests() {
  describe('Basic streaming', () => {
    it('should emit stream events in correct order', async () => {
      runner
        .when('agent-1')
        .streams([
          { type: 'text_delta', text: 'Thinking...' },
          { type: 'tool_use', name: 'bash', input: 'ls' },
          { type: 'tool_result', result: 'file1.txt\nfile2.txt' },
        ])
        .thenReturns({ done: true });

      const result = await runner.run('test task', { agentId: 'agent-1' });

      assert.strictEqual(result.success, true);
      assert.strictEqual(result.output, JSON.stringify({ done: true }));

      const events = runner.getStreamEvents('agent-1', 0);
      assert.strictEqual(events.length, 3);
      assert.strictEqual(events[0].type, 'text_delta');
      assert.strictEqual(events[0].text, 'Thinking...');
      assert.strictEqual(events[1].type, 'tool_use');
      assert.strictEqual(events[1].name, 'bash');
      assert.strictEqual(events[2].type, 'tool_result');
      assert.strictEqual(events[2].result, 'file1.txt\nfile2.txt');
    });

    it('should support string output in thenReturns', async () => {
      runner
        .when('agent-1')
        .streams([{ type: 'text_delta', text: 'Working...' }])
        .thenReturns('{"status": "complete"}');

      const result = await runner.run('test', { agentId: 'agent-1' });

      assert.strictEqual(result.success, true);
      assert.strictEqual(result.output, '{"status": "complete"}');
    });
  });
}

function defineEventDelayTests() {
  describe('Event delays', () => {
    it('should emit events with configurable delays', async () => {
      const delayMs = 100;
      const startTime = Date.now();

      runner
        .when('agent-1')
        .streams(
          [
            { type: 'text_delta', text: 'First' },
            { type: 'text_delta', text: 'Second' },
            { type: 'text_delta', text: 'Third' },
          ],
          delayMs
        )
        .thenReturns({ done: true });

      await runner.run('test', { agentId: 'agent-1' });

      const elapsed = Date.now() - startTime;

      // Should take at least 3 events * delay (allowing some timing variance)
      assert(elapsed >= delayMs * 2, `Expected at least ${delayMs * 2}ms, but took ${elapsed}ms`);
    });

    it('should support zero delay for instant emission', async () => {
      const startTime = Date.now();

      runner
        .when('agent-1')
        .streams(
          [
            { type: 'text_delta', text: '1' },
            { type: 'text_delta', text: '2' },
            { type: 'text_delta', text: '3' },
          ],
          0
        )
        .thenReturns({ done: true });

      await runner.run('test', { agentId: 'agent-1' });

      const elapsed = Date.now() - startTime;

      // Should complete quickly with zero delay
      assert(elapsed < 50, `Expected less than 50ms, but took ${elapsed}ms`);
    });
  });
}

function defineOnOutputCallbackTests() {
  describe('onOutput callback', () => {
    it('should call onOutput callback for each event', async () => {
      const capturedEvents = [];

      runner
        .when('agent-1')
        .streams(
          [
            { type: 'text_delta', text: 'Hello' },
            { type: 'tool_use', name: 'bash', input: 'echo test' },
            { type: 'tool_result', result: 'test' },
          ],
          0
        )
        .thenReturns({ done: true });

      await runner.run('test', {
        agentId: 'agent-1',
        onOutput: (content, agentId) => {
          capturedEvents.push({ content, agentId });
        },
      });

      assert.strictEqual(capturedEvents.length, 3);
      assert.strictEqual(capturedEvents[0].agentId, 'agent-1');
      assert.strictEqual(JSON.parse(capturedEvents[0].content).type, 'text_delta');
      assert.strictEqual(JSON.parse(capturedEvents[1].content).type, 'tool_use');
      assert.strictEqual(JSON.parse(capturedEvents[2].content).type, 'tool_result');
    });
  });
}

function defineMixedBehaviorTests() {
  describe('Mixed behaviors', () => {
    it('should support both streaming and non-streaming agents', async () => {
      // Streaming agent
      runner
        .when('streaming-agent')
        .streams([{ type: 'text_delta', text: 'Streaming...' }], 0)
        .thenReturns({ done: true });

      // Non-streaming agent
      runner.when('regular-agent').returns({ result: 'immediate' });

      const streamResult = await runner.run('test', {
        agentId: 'streaming-agent',
      });
      const regularResult = await runner.run('test', {
        agentId: 'regular-agent',
      });

      assert.strictEqual(streamResult.success, true);
      assert.strictEqual(regularResult.success, true);

      const streamEvents = runner.getStreamEvents('streaming-agent', 0);
      const regularEvents = runner.getStreamEvents('regular-agent', 0);

      assert.strictEqual(streamEvents.length, 1);
      assert.strictEqual(regularEvents.length, 0);
    });

    it('should track stream events separately per call', async () => {
      runner
        .when('agent-1')
        .streams([{ type: 'text_delta', text: 'First call' }], 0)
        .thenReturns({ done: true });

      // First call
      await runner.run('test 1', { agentId: 'agent-1' });

      // Second call (same config)
      await runner.run('test 2', { agentId: 'agent-1' });

      const events1 = runner.getStreamEvents('agent-1', 0);
      const events2 = runner.getStreamEvents('agent-1', 1);

      assert.strictEqual(events1.length, 1);
      assert.strictEqual(events2.length, 1);
      assert.strictEqual(events1[0].text, 'First call');
      assert.strictEqual(events2[0].text, 'First call');
    });
  });
}

function defineAssertStreamedEventsTests() {
  describe('assertStreamedEvents', () => {
    it('should assert events match expected', async () => {
      runner
        .when('agent-1')
        .streams(
          [
            { type: 'text_delta', text: 'Hello' },
            { type: 'tool_use', name: 'bash', input: 'ls' },
          ],
          0
        )
        .thenReturns({ done: true });

      await runner.run('test', { agentId: 'agent-1' });

      // Should not throw
      runner.assertStreamedEvents('agent-1', [
        { type: 'text_delta', text: 'Hello' },
        { type: 'tool_use', name: 'bash', input: 'ls' },
      ]);
    });

    it('should support partial matching with object subset', async () => {
      runner
        .when('agent-1')
        .streams(
          [
            { type: 'text_delta', text: 'Hello', timestamp: 123 },
            { type: 'tool_use', name: 'bash', input: 'ls', id: 'tool-1' },
          ],
          0
        )
        .thenReturns({ done: true });

      await runner.run('test', { agentId: 'agent-1' });

      // Should not throw - only checks specified keys
      runner.assertStreamedEvents('agent-1', [
        { type: 'text_delta', text: 'Hello' },
        { type: 'tool_use', name: 'bash' },
      ]);
    });

    it('should fail if event count does not match', async () => {
      runner
        .when('agent-1')
        .streams([{ type: 'text_delta', text: 'Hello' }], 0)
        .thenReturns({ done: true });

      await runner.run('test', { agentId: 'agent-1' });

      assert.throws(() => {
        runner.assertStreamedEvents('agent-1', [
          { type: 'text_delta', text: 'Hello' },
          { type: 'text_delta', text: 'World' },
        ]);
      }, /Expected 2 stream events, but got 1/);
    });

    it('should fail if event type does not match', async () => {
      runner
        .when('agent-1')
        .streams([{ type: 'text_delta', text: 'Hello' }], 0)
        .thenReturns({ done: true });

      await runner.run('test', { agentId: 'agent-1' });

      assert.throws(() => {
        runner.assertStreamedEvents('agent-1', [{ type: 'tool_use', name: 'bash' }]);
      }, /Expected event 0\.type to be "tool_use"/);
    });

    it('should fail if event property does not match', async () => {
      runner
        .when('agent-1')
        .streams([{ type: 'text_delta', text: 'Hello' }], 0)
        .thenReturns({ done: true });

      await runner.run('test', { agentId: 'agent-1' });

      assert.throws(() => {
        runner.assertStreamedEvents('agent-1', [{ type: 'text_delta', text: 'World' }]);
      }, /Expected event 0\.text to be "World", but got "Hello"/);
    });

    it('should support asserting specific call index', async () => {
      runner
        .when('agent-1')
        .streams([{ type: 'text_delta', text: 'First' }], 0)
        .thenReturns({ done: true });

      await runner.run('test 1', { agentId: 'agent-1' });
      await runner.run('test 2', { agentId: 'agent-1' });

      // Assert first call
      runner.assertStreamedEvents('agent-1', [{ type: 'text_delta', text: 'First' }], 0);

      // Assert second call
      runner.assertStreamedEvents('agent-1', [{ type: 'text_delta', text: 'First' }], 1);
    });
  });
}

function defineGetStreamEventsTests() {
  describe('getStreamEvents', () => {
    it('should return empty array for non-existent call', () => {
      const events = runner.getStreamEvents('non-existent-agent', 0);
      assert.deepStrictEqual(events, []);
    });

    it('should return empty array for out-of-bounds call index', async () => {
      runner
        .when('agent-1')
        .streams([{ type: 'text_delta', text: 'Hello' }], 0)
        .thenReturns({ done: true });

      await runner.run('test', { agentId: 'agent-1' });

      const events = runner.getStreamEvents('agent-1', 5);
      assert.deepStrictEqual(events, []);
    });

    it('should return events for specific call', async () => {
      runner
        .when('agent-1')
        .streams([{ type: 'text_delta', text: 'Hello' }], 0)
        .thenReturns({ done: true });

      await runner.run('test', { agentId: 'agent-1' });

      const events = runner.getStreamEvents('agent-1', 0);
      assert.strictEqual(events.length, 1);
      assert.strictEqual(events[0].type, 'text_delta');
    });
  });
}

function defineRealWorldUseCaseTests() {
  describe('Real-world use cases', () => {
    it('should simulate Claude task execution with multiple event types', async () => {
      runner
        .when('worker')
        .streams(
          [
            { type: 'text_delta', text: 'Analyzing the issue...' },
            { type: 'tool_use', name: 'read', input: '/path/to/file.js' },
            { type: 'tool_result', result: 'function foo() { ... }' },
            { type: 'text_delta', text: 'Found the bug. Fixing...' },
            {
              type: 'tool_use',
              name: 'edit',
              input: { file: '/path/to/file.js', changes: '...' },
            },
            { type: 'tool_result', result: 'File updated successfully' },
            { type: 'text_delta', text: 'Fix complete!' },
          ],
          50
        )
        .thenReturns({
          summary: 'Fixed null pointer bug in foo()',
          filesChanged: ['/path/to/file.js'],
        });

      const streamedEvents = [];
      const result = await runner.run('Fix the bug in foo()', {
        agentId: 'worker',
        onOutput: (content) => {
          streamedEvents.push(JSON.parse(content));
        },
      });

      assert.strictEqual(result.success, true);
      assert.strictEqual(streamedEvents.length, 7);
      assert.strictEqual(streamedEvents[0].type, 'text_delta');
      assert.strictEqual(streamedEvents[1].type, 'tool_use');
      assert.strictEqual(streamedEvents[1].name, 'read');
      assert.strictEqual(streamedEvents[6].text, 'Fix complete!');

      const outputData = JSON.parse(result.output);
      assert.strictEqual(outputData.summary, 'Fixed null pointer bug in foo()');
      assert.strictEqual(outputData.filesChanged.length, 1);
    });

    it('should support validation workflow with streamed feedback', async () => {
      runner
        .when('validator')
        .streams(
          [
            { type: 'text_delta', text: 'Reviewing changes...' },
            { type: 'tool_use', name: 'bash', input: 'npm test' },
            { type: 'tool_result', result: 'Tests passed: 42/42' },
            { type: 'text_delta', text: 'All checks passed!' },
          ],
          30
        )
        .thenReturns({ approved: true, tests_passed: 42 });

      await runner.run('Validate the implementation', { agentId: 'validator' });

      runner.assertStreamedEvents('validator', [
        { type: 'text_delta' },
        { type: 'tool_use', name: 'bash' },
        { type: 'tool_result' },
        { type: 'text_delta' },
      ]);
    });
  });
}

function defineResetTests() {
  describe('reset()', () => {
    it('should clear stream events on reset', async () => {
      runner
        .when('agent-1')
        .streams([{ type: 'text_delta', text: 'Hello' }], 0)
        .thenReturns({ done: true });

      await runner.run('test', { agentId: 'agent-1' });

      assert.strictEqual(runner.getStreamEvents('agent-1', 0).length, 1);

      runner.reset();

      assert.strictEqual(runner.getStreamEvents('agent-1', 0).length, 0);
    });
  });
}
