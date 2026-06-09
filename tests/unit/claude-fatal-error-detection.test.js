const { describe, it } = require('mocha');
const { expect } = require('chai');

const { NO_MESSAGES_RETURNED, detectProviderFatalError } = require('../../lib/agent-cli-provider');

describe('Claude fatal error detection', () => {
  it('detects "No messages returned" in error output', () => {
    const line = 'Error: No messages returned';

    const detected = detectProviderFatalError('claude', line);

    expect(detected).to.equal(`Claude CLI error: ${NO_MESSAGES_RETURNED}`);
  });

  it('is case-insensitive', () => {
    const line = 'error: NO MESSAGES RETURNED';

    const detected = detectProviderFatalError('claude', line);

    expect(detected).to.equal(`Claude CLI error: ${NO_MESSAGES_RETURNED}`);
  });

  it('returns null for unrelated output', () => {
    expect(detectProviderFatalError('claude', 'All good')).to.equal(null);
    expect(detectProviderFatalError('claude', '')).to.equal(null);
    expect(detectProviderFatalError('codex', 'Error: No messages returned')).to.equal(null);
  });

  it('does not flag valid JSON output that contains the message', () => {
    const jsonLine = JSON.stringify({
      type: 'result',
      structured_output: {
        summary: 'No messages returned in issue description',
      },
    });

    expect(detectProviderFatalError('claude', jsonLine)).to.equal(null);
  });
});
