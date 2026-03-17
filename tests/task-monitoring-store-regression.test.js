const assert = require('assert');
const fs = require('fs');
const path = require('path');

describe('task monitoring store regression', () => {
  it('agent-task-executor polls task store instead of spawning zeroshot status', () => {
    const source = fs.readFileSync(
      path.join(__dirname, '..', 'src', 'agent', 'agent-task-executor.js'),
      'utf8'
    );

    const waitForTaskReady = source.match(/async function waitForTaskReady[\s\S]*?^}/m)?.[0] || '';
    const createLogFollower = source.match(/function createLogFollower[\s\S]*?^}/m)?.[0] || '';

    assert.ok(
      waitForTaskReady.includes('getTask(taskId)'),
      'waitForTaskReady should read task store'
    );
    assert.ok(
      !waitForTaskReady.includes('status ${taskId}'),
      'waitForTaskReady must not shell out to zeroshot status'
    );
    assert.ok(
      createLogFollower.includes('readTaskMonitorState(taskId)'),
      'log follower should use direct task-store monitoring'
    );
    assert.ok(
      !createLogFollower.includes('exec(`${ctPath} status ${taskId}`'),
      'log follower must not shell out to zeroshot status'
    );
  });

  it('claude-task-runner polls task store instead of spawning zeroshot status', () => {
    const source = fs.readFileSync(
      path.join(__dirname, '..', 'src', 'claude-task-runner.js'),
      'utf8'
    );

    const waitForTaskReady = source.match(/async _waitForTaskReady[\s\S]*?^\s{2}}/m)?.[0] || '';
    const followLogsStart = source.indexOf('  _followLogs(ctPath, taskId, agentId) {');
    const followLogsEnd = source.indexOf(
      '\n  /**\n   * Run task in isolated Docker container',
      followLogsStart
    );
    const followLogs =
      followLogsStart >= 0 && followLogsEnd > followLogsStart
        ? source.slice(followLogsStart, followLogsEnd)
        : '';

    assert.ok(
      waitForTaskReady.includes('getTask(taskId)'),
      '_waitForTaskReady should read task store'
    );
    assert.ok(
      !waitForTaskReady.includes('status ${taskId}'),
      '_waitForTaskReady must not shell out to zeroshot status'
    );
    assert.ok(
      followLogs.includes('readTaskMonitorState(taskId)'),
      '_followLogs should use direct task-store monitoring'
    );
    assert.ok(
      !followLogs.includes('exec(`${ctPath} status ${taskId}`'),
      '_followLogs must not shell out to zeroshot status'
    );
  });

  it('agent-task-executor sends task prompts over stdin instead of argv', () => {
    const source = fs.readFileSync(
      path.join(__dirname, '..', 'src', 'agent', 'agent-task-executor.js'),
      'utf8'
    );

    assert.ok(source.includes("'--stdin'"), 'agent-task-executor should enable stdin prompt mode');
    assert.ok(
      source.includes("proc.stdin.end(input, 'utf8')"),
      'agent-task-executor should write prompt text to stdin'
    );
  });

  it('claude-task-runner sends task prompts over stdin instead of argv', () => {
    const source = fs.readFileSync(
      path.join(__dirname, '..', 'src', 'claude-task-runner.js'),
      'utf8'
    );

    assert.ok(source.includes("'--stdin'"), 'claude-task-runner should enable stdin prompt mode');
    assert.ok(
      source.includes("proc.stdin.end(input, 'utf8')"),
      'claude-task-runner should write prompt text to stdin'
    );
  });
});
