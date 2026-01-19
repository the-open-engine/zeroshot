const assert = require('assert');
const { parseProviderChunk } = require('../../src/providers');

describe('Provider chunk parsing', () => {
  it('strips timestamp prefixes before parsing', () => {
    const line =
      '[1721088000000]' +
      JSON.stringify({
        type: 'stream_event',
        event: {
          type: 'content_block_delta',
          delta: { type: 'text_delta', text: 'Hi' },
        },
      });
    const events = parseProviderChunk('claude', `${line}\n`);

    assert.strictEqual(events.length, 1);
    assert.strictEqual(events[0].type, 'text');
    assert.strictEqual(events[0].text, 'Hi');
  });
});
