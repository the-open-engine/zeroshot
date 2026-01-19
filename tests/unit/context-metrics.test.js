const assert = require('assert');
const { buildContextMetrics, estimateTokensFromChars } = require('../../src/agent/context-metrics');

describe('Context Metrics', function () {
  it('estimates tokens using ceil(chars / 4)', function () {
    assert.strictEqual(estimateTokensFromChars(0), 0);
    assert.strictEqual(estimateTokensFromChars(1), 1);
    assert.strictEqual(estimateTokensFromChars(4), 1);
    assert.strictEqual(estimateTokensFromChars(5), 2);
  });

  it('builds section breakdown with totals', function () {
    const metrics = buildContextMetrics({
      clusterId: 'cluster-1',
      agentId: 'agent-1',
      role: 'worker',
      iteration: 2,
      triggeringMessage: { topic: 'TASK', sender: 'user' },
      strategy: { sources: [{ topic: 'A' }, { topic: 'B' }], maxTokens: 10 },
      sections: {
        header: 'abcd',
        instructions: '12345',
        legacyOutputSchema: '',
        jsonSchema: 'xy',
        sources: '',
        validatorSkip: 'z',
        triggeringMessage: 'q',
      },
    });

    assert.strictEqual(metrics.sections.header.chars, 4);
    assert.strictEqual(metrics.sections.header.estimatedTokens, 1);
    assert.strictEqual(metrics.sections.instructions.chars, 5);
    assert.strictEqual(metrics.sections.instructions.estimatedTokens, 2);
    assert.strictEqual(metrics.sections.jsonSchema.chars, 2);
    assert.strictEqual(metrics.sections.validatorSkip.chars, 1);
    assert.strictEqual(metrics.sections.triggeringMessage.chars, 1);
    assert.strictEqual(metrics.total.chars, 4 + 5 + 0 + 2 + 0 + 1 + 1);
    assert.strictEqual(metrics.total.estimatedTokens, Math.ceil(metrics.total.chars / 4));

    assert.strictEqual(metrics.strategy.maxTokens, 10);
    assert.strictEqual(metrics.strategy.sourcesCount, 2);
    assert.strictEqual(metrics.triggeredBy, 'TASK');
    assert.strictEqual(metrics.triggerFrom, 'user');
    assert.strictEqual(metrics.truncation.maxContextChars.beforeChars, metrics.total.chars);
    assert.strictEqual(metrics.truncation.legacyMaxTokens.maxTokens, 10);
  });

  it('defaults maxTokens when not provided', function () {
    const metrics = buildContextMetrics({
      clusterId: 'cluster-1',
      agentId: 'agent-1',
      role: 'worker',
      iteration: 1,
      triggeringMessage: { topic: 'TASK', sender: 'user' },
      strategy: { sources: [] },
      sections: { header: 'a' },
    });

    assert.strictEqual(metrics.strategy.maxTokens, 100000);
  });
});
