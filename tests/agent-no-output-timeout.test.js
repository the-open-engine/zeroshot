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
    assert.ok(sourceCode.includes("'AGENT_ERROR'"), 'Expected AGENT_ERROR publication');
    assert.ok(sourceCode.includes("'no_output_timeout'"), 'Expected no_output_timeout error code');
  });

  it('kills task and resolves failure when timeout is exceeded', () => {
    assert.ok(sourceCode.includes('task kill'), 'Expected task kill command for stuck task');
    assert.ok(sourceCode.includes('success: false'), 'Expected failure resolution on timeout');
    assert.ok(
      sourceCode.includes('No messages returned'),
      'Expected explicit no-output error text'
    );
  });

  it('wires no-output timeout check into log follower loop', () => {
    assert.ok(
      sourceCode.includes('handleNoOutputTimeout({'),
      'Expected createLogFollower to call handleNoOutputTimeout before status polling'
    );
  });

  it('uses process health analysis to defer false timeouts', () => {
    assert.ok(
      sourceCode.includes('analyzeProcessHealth'),
      'Expected no-output timeout path to run process-health analysis'
    );
    assert.ok(
      sourceCode.includes("'no_output_timeout_deferred'"),
      'Expected defer signal when process appears active'
    );
  });
});
