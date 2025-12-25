/**
 * Example: Using MockTaskRunner with streaming support
 *
 * This demonstrates how to test Claude task execution with simulated streaming events.
 */

const MockTaskRunner = require('../helpers/mock-task-runner');

async function exampleBasicStreaming() {
  const runner = new MockTaskRunner();

  // Configure agent with streaming behavior
  runner
    .when('debugger-agent')
    .streams(
      [
        { type: 'text_delta', text: 'Reading source file...' },
        { type: 'tool_use', name: 'read', input: '/src/app.js' },
        { type: 'tool_result', result: 'function buggyCode() { ... }' },
        { type: 'text_delta', text: 'Found the issue! Fixing...' },
        { type: 'tool_use', name: 'edit', input: { file: '/src/app.js' } },
        { type: 'tool_result', result: 'File updated' },
        { type: 'text_delta', text: 'Fix complete!' },
      ],
      100
    ) // 100ms between events
    .thenReturns({
      success: true,
      summary: 'Fixed null pointer bug',
      filesChanged: ['/src/app.js'],
    });

  // Execute the task
  const result = await runner.run('Debug the application', {
    agentId: 'debugger-agent',
  });

  console.log('Result:', result);

  // Assert stream events
  runner.assertStreamedEvents('debugger-agent', [
    { type: 'text_delta', text: 'Reading source file...' },
    { type: 'tool_use', name: 'read' },
    { type: 'tool_result' },
    { type: 'text_delta' },
    { type: 'tool_use', name: 'edit' },
    { type: 'tool_result' },
    { type: 'text_delta', text: 'Fix complete!' },
  ]);

  console.log('Assertions passed!');
}

async function exampleWithCallback() {
  const runner = new MockTaskRunner();
  const capturedEvents = [];

  runner
    .when('validator')
    .streams(
      [
        { type: 'text_delta', text: 'Running tests...' },
        { type: 'tool_use', name: 'bash', input: 'npm test' },
        { type: 'tool_result', result: 'All tests passed' },
      ],
      0
    )
    .thenReturns({ approved: true });

  // Capture streaming events via callback
  await runner.run('Validate implementation', {
    agentId: 'validator',
    onOutput: (content, agentId) => {
      const event = JSON.parse(content);
      capturedEvents.push(event);
      console.log(`[${agentId}] ${event.type}:`, event.text || event.name || event.result);
    },
  });

  console.log('Captured events:', capturedEvents);
}

async function exampleMixedBehaviors() {
  const runner = new MockTaskRunner();

  // Streaming agent
  runner
    .when('streaming-worker')
    .streams([{ type: 'text_delta', text: 'Processing...' }], 0)
    .thenReturns({ done: true });

  // Non-streaming agent
  runner.when('instant-worker').returns({ result: 'immediate' });

  await runner.run('task 1', { agentId: 'streaming-worker' });
  await runner.run('task 2', { agentId: 'instant-worker' });

  console.log('Streaming events:', runner.getStreamEvents('streaming-worker', 0));
  console.log('Non-streaming events:', runner.getStreamEvents('instant-worker', 0));
}

// Run examples
(async () => {
  console.log('\n=== Basic Streaming ===');
  await exampleBasicStreaming();

  console.log('\n=== Streaming with Callback ===');
  await exampleWithCallback();

  console.log('\n=== Mixed Behaviors ===');
  await exampleMixedBehaviors();
})();
