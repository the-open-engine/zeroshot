const { describe, it } = require('mocha');
const { expect } = require('chai');
const path = require('path');
const { pathToFileURL } = require('url');

function loadRecoveryModule() {
  const modulePath = path.resolve(__dirname, '../../task-lib/claude-recovery.js');
  return import(pathToFileURL(modulePath).href);
}

describe('Claude fatal error detection', () => {
  it('detects "No messages returned" in error output', async () => {
    const { detectFatalClaudeError, NO_MESSAGES_RETURNED } = await loadRecoveryModule();
    const line = 'Error: No messages returned';

    const detected = detectFatalClaudeError(line);

    expect(detected).to.equal(`Claude CLI error: ${NO_MESSAGES_RETURNED}`);
  });

  it('is case-insensitive', async () => {
    const { detectFatalClaudeError, NO_MESSAGES_RETURNED } = await loadRecoveryModule();
    const line = 'error: NO MESSAGES RETURNED';

    const detected = detectFatalClaudeError(line);

    expect(detected).to.equal(`Claude CLI error: ${NO_MESSAGES_RETURNED}`);
  });

  it('returns null for unrelated output', async () => {
    const { detectFatalClaudeError } = await loadRecoveryModule();

    expect(detectFatalClaudeError('All good')).to.equal(null);
    expect(detectFatalClaudeError('')).to.equal(null);
  });

  it('does not flag valid JSON output that contains the message', async () => {
    const { detectFatalClaudeError } = await loadRecoveryModule();
    const jsonLine = JSON.stringify({
      type: 'result',
      structured_output: {
        summary: 'No messages returned in issue description',
      },
    });

    expect(detectFatalClaudeError(jsonLine)).to.equal(null);
  });
});
