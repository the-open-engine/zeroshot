const assert = require('assert');
const {
  extractJsonFromOutput,
  hasFatalStandaloneOutput,
} = require('../../src/agent/output-extraction');

describe('Output Extraction - fatal strings handling', () => {
  it('extracts JSON when fatal substrings appear inside JSON events', () => {
    const output = [
      '{"type":"item.completed","item":{"id":"item_1","type":"command_execution","aggregated_output":"Task not found"}}',
      '{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"{\\"summary\\":\\"ok\\",\\"completionStatus\\":{\\"canValidate\\":true,\\"percentComplete\\":100}}"}}',
      '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}',
    ].join('\n');

    const parsed = extractJsonFromOutput(output, 'codex');

    assert.deepStrictEqual(parsed, {
      summary: 'ok',
      completionStatus: {
        canValidate: true,
        percentComplete: 100,
      },
    });
  });

  it('detects standalone fatal output lines', () => {
    const output = 'Task not found\n';
    assert.strictEqual(hasFatalStandaloneOutput(output), true);
  });
});
