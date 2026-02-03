const assert = require('assert');
const { parseChunk } = require('../../lib/stream-json-parser');

describe('stream-json parser (Codex)', () => {
  it('maps command_execution start/completed into tool_call/tool_result', () => {
    const chunk = [
      JSON.stringify({
        type: 'item.started',
        item: { id: 'item_1', type: 'command_execution', command: 'ls -la' },
      }),
      JSON.stringify({
        type: 'item.completed',
        item: {
          id: 'item_1',
          type: 'command_execution',
          aggregated_output: 'file1.txt\nfile2.txt\n',
          exit_code: 0,
        },
      }),
    ].join('\n');

    const events = parseChunk(chunk);
    assert.deepStrictEqual(events[0], {
      type: 'tool_call',
      toolName: 'Bash',
      toolId: 'item_1',
      input: { command: 'ls -la' },
    });
    assert.deepStrictEqual(events[1], {
      type: 'tool_result',
      toolId: 'item_1',
      content: 'file1.txt\nfile2.txt\n',
      isError: false,
    });
  });

  it('maps reasoning items into thinking', () => {
    const chunk = JSON.stringify({
      type: 'item.completed',
      item: { id: 'r1', type: 'reasoning', text: 'thinking...' },
    });
    const events = parseChunk(chunk);
    assert.deepStrictEqual(events, [{ type: 'thinking', text: 'thinking...' }]);
  });

  it('maps top-level errors into result errors', () => {
    const chunk = JSON.stringify({ type: 'error', error: { message: 'boom' } });
    const events = parseChunk(chunk);
    assert.deepStrictEqual(events, [{ type: 'result', success: false, error: 'boom' }]);
  });
});
