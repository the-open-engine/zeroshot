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
      packs: [
        { id: 'header', section: 'header', status: 'included', chars: 4, estimatedTokens: 1 },
        {
          id: 'instructions',
          section: 'instructions',
          status: 'included',
          chars: 5,
          estimatedTokens: 2,
        },
        {
          id: 'jsonSchema',
          section: 'jsonSchema',
          status: 'included',
          chars: 2,
          estimatedTokens: 1,
        },
        {
          id: 'validatorSkip',
          section: 'validatorSkip',
          status: 'included',
          chars: 1,
          estimatedTokens: 1,
        },
        {
          id: 'triggeringMessage',
          section: 'triggeringMessage',
          status: 'included',
          chars: 1,
          estimatedTokens: 1,
        },
        {
          id: 'sources-a',
          section: 'sources',
          status: 'included',
          chars: 3,
          estimatedTokens: 1,
        },
        {
          id: 'sources-skipped',
          section: 'sources',
          status: 'skipped',
          chars: 100,
          estimatedTokens: 25,
        },
      ],
      budget: {
        maxTokens: 10,
        remainingTokens: 2,
        overBudgetTokens: 0,
        finalTokens: 6,
      },
    });

    assert.strictEqual(metrics.sections.header.chars, 4);
    assert.strictEqual(metrics.sections.header.estimatedTokens, 1);
    assert.strictEqual(metrics.sections.instructions.chars, 5);
    assert.strictEqual(metrics.sections.instructions.estimatedTokens, 2);
    assert.strictEqual(metrics.sections.jsonSchema.chars, 2);
    assert.strictEqual(metrics.sections.validatorSkip.chars, 1);
    assert.strictEqual(metrics.sections.triggeringMessage.chars, 1);
    assert.strictEqual(metrics.sections.sources.chars, 3);
    assert.strictEqual(metrics.total.chars, 4 + 5 + 2 + 1 + 1 + 3);
    assert.strictEqual(metrics.total.estimatedTokens, Math.ceil(metrics.total.chars / 4));

    assert.strictEqual(metrics.strategy.maxTokens, 10);
    assert.strictEqual(metrics.strategy.sourcesCount, 2);
    assert.strictEqual(metrics.triggeredBy, 'TASK');
    assert.strictEqual(metrics.triggerFrom, 'user');
    assert.strictEqual(metrics.budget.maxTokens, 10);
    assert.strictEqual(metrics.budget.remainingTokens, 2);
    assert.strictEqual(metrics.truncation.maxContextChars.beforeChars, metrics.total.chars);
  });

  it('defaults maxTokens when not provided', function () {
    const metrics = buildContextMetrics({
      clusterId: 'cluster-1',
      agentId: 'agent-1',
      role: 'worker',
      iteration: 1,
      triggeringMessage: { topic: 'TASK', sender: 'user' },
      strategy: { sources: [] },
      packs: [{ id: 'header', section: 'header', status: 'included', chars: 1 }],
    });

    assert.strictEqual(metrics.strategy.maxTokens, 100000);
    assert.strictEqual(metrics.budget.maxTokens, 100000);
  });
});
