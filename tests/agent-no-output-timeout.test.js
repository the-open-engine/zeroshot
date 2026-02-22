/**
 * TEST: Agent no-output timeout fail-fast behavior
 *
 * Verifies that agent-task-executor contains explicit no-output timeout handling
 * so stuck provider subprocesses fail and retry instead of hanging indefinitely.
 */

const assert = require('assert');
const fs = require('fs');
const path = require('path');

describe('Agent no-output timeout fail-fast', () => {
  const sourceFile = path.join(__dirname, '..', 'src', 'agent', 'agent-task-executor.js');
  let sourceCode;

  before(() => {
    sourceCode = fs.readFileSync(sourceFile, 'utf8');
  });

  it('defines handleNoOutputTimeout helper', () => {
    assert.ok(
      sourceCode.includes('function handleNoOutputTimeout'),
      'Expected handleNoOutputTimeout helper to exist'
    );
  });

  it('publishes AGENT_ERROR with no_output_timeout code', () => {
    const fnMatch = sourceCode.match(/function handleNoOutputTimeout\([^)]*\)\s*{[\s\S]*?^}/m);
    assert.ok(fnMatch, 'Expected to find handleNoOutputTimeout function body');

    const fnBody = fnMatch[0];
    assert.ok(fnBody.includes("'AGENT_ERROR'"), 'Expected AGENT_ERROR publication');
    assert.ok(fnBody.includes("'no_output_timeout'"), 'Expected no_output_timeout error code');
  });

  it('kills task and resolves failure when timeout is exceeded', () => {
    const fnMatch = sourceCode.match(/function handleNoOutputTimeout\([^)]*\)\s*{[\s\S]*?^}/m);
    assert.ok(fnMatch, 'Expected to find handleNoOutputTimeout function body');

    const fnBody = fnMatch[0];
    assert.ok(fnBody.includes('task kill'), 'Expected task kill command for stuck task');
    assert.ok(fnBody.includes('success: false'), 'Expected failure resolution on timeout');
    assert.ok(fnBody.includes('No messages returned'), 'Expected explicit no-output error text');
  });

  it('wires no-output timeout check into log follower loop', () => {
    assert.ok(
      sourceCode.includes('if (handleNoOutputTimeout({ agent, state, ctPath, taskId, resolve, noOutputTimeoutMs }))'),
      'Expected createLogFollower to call handleNoOutputTimeout before status polling'
    );
  });
});
