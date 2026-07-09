const assert = require('assert');

const { renderRecentMessagesToTerminal } = require('../../cli/index.js');

function message(overrides) {
  return {
    id: overrides.id || `msg-${overrides.timestamp || 0}`,
    timestamp: overrides.timestamp || 0,
    topic: overrides.topic,
    sender: overrides.sender || 'worker',
    cluster_id: overrides.cluster_id || 'cluster-1',
    content: overrides.content || {},
    ...overrides,
  };
}

function codexLine(payload) {
  return {
    text: JSON.stringify(payload),
    data: {
      line: JSON.stringify(payload),
      provider: 'codex',
      fromTaskLog: true,
    },
  };
}

describe('cli logs history regressions', function () {
  it('limits after selecting printable history so noisy tails do not hide useful events', function () {
    const messages = [
      message({
        topic: 'VALIDATION_RESULT',
        timestamp: 1,
        sender: 'validator',
        content: {
          data: {
            approved: false,
            summary: 'missing edge-case test',
            issues: ['edge case missing'],
          },
        },
      }),
    ];

    for (let i = 0; i < 60; i++) {
      messages.push(
        message({
          id: `empty-agent-output-${i}`,
          topic: 'AGENT_OUTPUT',
          timestamp: 2 + i,
          content: { text: '   ' },
        })
      );
    }

    const rendered = renderRecentMessagesToTerminal(messages, 50, { isActive: false });

    assert(rendered.includes('VALIDATION_RESULT'));
    assert(rendered.includes('missing edge-case test'));
  });

  it('renders historical Codex AGENT_OUTPUT text and success result summaries', function () {
    const messages = [
      message({
        id: 'codex-text',
        topic: 'AGENT_OUTPUT',
        timestamp: 1,
        sender_provider: 'codex',
        content: codexLine({
          type: 'item.completed',
          item: { type: 'agent_message', id: 'item_1', text: 'hello from codex' },
        }),
      }),
      message({
        id: 'codex-result',
        topic: 'AGENT_OUTPUT',
        timestamp: 2,
        sender_provider: 'codex',
        content: codexLine({
          type: 'turn.completed',
          usage: { input_tokens: 7, output_tokens: 3 },
        }),
      }),
    ];

    const rendered = renderRecentMessagesToTerminal(messages, 50, { isActive: false });

    assert(rendered.includes('hello from codex'));
    assert(rendered.includes('completed (7 in, 3 out)'));
  });
});
