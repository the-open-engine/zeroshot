const assert = require('assert');
const { getProvider } = require('../../src/providers');

describe('Provider error retry classification', () => {
  it('retries on rate limit (all providers)', () => {
    const err = new Error('Rate limit exceeded. Retry after 60 seconds.');
    for (const name of ['claude', 'codex', 'gemini']) {
      const provider = getProvider(name);
      assert.strictEqual(provider.isRetryableError(err), true, `${name} should retry rate limit`);
    }
  });

  it('does not retry on invalid_api_key (all providers)', () => {
    const err = new Error('invalid_api_key: key revoked');
    for (const name of ['claude', 'codex', 'gemini']) {
      const provider = getProvider(name);
      assert.strictEqual(provider.isRetryableError(err), false, `${name} should not retry auth`);
    }
  });

  it('Claude: retries on "No messages returned"', () => {
    const provider = getProvider('claude');
    assert.strictEqual(provider.isRetryableError(new Error('No messages returned')), true);
  });

  it('Codex: retries on "server_error"', () => {
    const provider = getProvider('codex');
    assert.strictEqual(provider.isRetryableError(new Error('server_error')), true);
  });

  it('Gemini: retries on "RESOURCE_EXHAUSTED"', () => {
    const provider = getProvider('gemini');
    assert.strictEqual(provider.isRetryableError(new Error('RESOURCE_EXHAUSTED')), true);
  });
});
