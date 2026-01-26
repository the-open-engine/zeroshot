const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');

const Ledger = require('../../src/ledger');
const MessageBus = require('../../src/message-bus');
const { buildContext } = require('../../src/agent/agent-context-builder');

const MAX_CONTEXT_CHARS = 500000;

describe('Context Metrics Integration', function () {
  let tempDir;
  let ledger;
  let messageBus;
  let originalMetricsEnv;
  let originalLedgerEnv;

  beforeEach(() => {
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-context-metrics-'));
    const dbPath = path.join(tempDir, 'test-ledger.db');
    ledger = new Ledger(dbPath);
    messageBus = new MessageBus(ledger);

    originalMetricsEnv = process.env.ZEROSHOT_CONTEXT_METRICS;
    originalLedgerEnv = process.env.ZEROSHOT_CONTEXT_METRICS_LEDGER;
    process.env.ZEROSHOT_CONTEXT_METRICS = '0';
    process.env.ZEROSHOT_CONTEXT_METRICS_LEDGER = '1';
  });

  afterEach(() => {
    if (tempDir && fs.existsSync(tempDir)) {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }

    if (originalMetricsEnv === undefined) {
      delete process.env.ZEROSHOT_CONTEXT_METRICS;
    } else {
      process.env.ZEROSHOT_CONTEXT_METRICS = originalMetricsEnv;
    }

    if (originalLedgerEnv === undefined) {
      delete process.env.ZEROSHOT_CONTEXT_METRICS_LEDGER;
    } else {
      process.env.ZEROSHOT_CONTEXT_METRICS_LEDGER = originalLedgerEnv;
    }
  });

  it('publishes metrics to the ledger and records truncation stages', function () {
    const clusterId = 'cluster-metrics-1';
    const createdAt = Date.now() - 60000;
    const secretMarker = 'SECRET_CONTEXT_PAYLOAD';
    const hugeText = `${secretMarker}${'x'.repeat(MAX_CONTEXT_CHARS + 200000)}`;

    messageBus.publish({
      cluster_id: clusterId,
      topic: 'ISSUE_OPENED',
      sender: 'user',
      content: { text: hugeText },
    });

    messageBus.publish({
      cluster_id: clusterId,
      topic: 'HUGE_TOPIC',
      sender: 'tester',
      content: { text: 'optional-context' },
    });

    const context = buildContext({
      id: 'worker',
      role: 'implementation',
      iteration: 1,
      config: {
        contextStrategy: {
          sources: [
            { topic: 'ISSUE_OPENED', since: 'cluster_start', limit: 1 },
            { topic: 'HUGE_TOPIC', since: 'cluster_start', limit: 1 },
          ],
          maxTokens: 1,
        },
      },
      messageBus,
      cluster: { id: clusterId, createdAt },
      triggeringMessage: {
        topic: 'TASK_READY',
        sender: 'planner',
        content: { text: 'go' },
      },
    });

    assert.ok(context.length > 0, 'Context should be generated');

    const metricsMessages = ledger.query({ cluster_id: clusterId, topic: 'CONTEXT_METRICS' });
    assert.strictEqual(metricsMessages.length, 1, 'Should publish one CONTEXT_METRICS message');

    const metrics = metricsMessages[0].content.data;
    assert.strictEqual(metrics.clusterId, clusterId);
    assert.strictEqual(metrics.agentId, 'worker');
    assert.strictEqual(metrics.role, 'implementation');
    assert.strictEqual(metrics.iteration, 1);
    assert.strictEqual(metrics.strategy.maxTokens, 1);
    assert.strictEqual(metrics.strategy.sourcesCount, 2);

    assert.strictEqual(metrics.truncation.maxContextChars.applied, true);
    assert.ok(
      metrics.truncation.maxContextChars.beforeChars >
        metrics.truncation.maxContextChars.afterChars,
      'Max context truncation should reduce size'
    );
    assert.ok(
      metrics.truncation.maxContextChars.afterChars <= MAX_CONTEXT_CHARS,
      'Final context should respect max char limit'
    );
    assert.strictEqual(metrics.budget.maxTokens, 1);
    assert.strictEqual(metrics.total.chars, context.length);

    const metricsJson = JSON.stringify(metrics);
    assert.ok(!metricsJson.includes(secretMarker), 'Metrics should not include raw context');
    assert.ok(metrics.sections.sources.chars > 0, 'Sources section should be counted');
    assert.ok(
      metrics.packs.some(
        (pack) => pack.id.startsWith('source:ISSUE_OPENED') && pack.status === 'included'
      ),
      'Required issue pack should be included'
    );
    assert.ok(
      metrics.packs.some(
        (pack) => pack.id.startsWith('source:HUGE_TOPIC') && pack.status === 'skipped'
      ),
      'Optional pack should be skipped when over budget'
    );
  });
});
