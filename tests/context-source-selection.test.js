const assert = require('assert');
const AgentWrapper = require('../src/agent-wrapper');
const MessageBus = require('../src/message-bus');
const Ledger = require('../src/ledger');
const path = require('path');
const fs = require('fs');
const os = require('os');

describe('context source selection', () => {
  let tempDir;
  let ledger;
  let messageBus;
  let clusterId;
  let clusterCreatedAt;

  beforeEach(() => {
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-test-'));
    const dbPath = path.join(tempDir, 'test-ledger.db');

    ledger = new Ledger(dbPath);
    messageBus = new MessageBus(ledger);

    clusterId = 'test-cluster-456';
    clusterCreatedAt = Date.now();
  });

  afterEach(() => {
    if (ledger) ledger.close();
    try {
      fs.rmSync(tempDir, { recursive: true, force: true });
    } catch {
      // Ignore cleanup errors
    }
  });

  function createWorker(contextStrategy) {
    const workerConfig = {
      id: 'worker',
      role: 'implementation',
      modelLevel: 'level2',
      timeout: 0,
      contextStrategy,
    };

    const mockCluster = {
      id: clusterId,
      createdAt: clusterCreatedAt,
      agents: [],
    };

    return new AgentWrapper(workerConfig, messageBus, mockCluster, {
      testMode: true,
      mockSpawnFn: () => {},
    });
  }

  function publishMessage(topic, text, timestamp) {
    messageBus.publish({
      cluster_id: clusterId,
      topic,
      sender: 'system',
      content: { text },
      timestamp,
    });
  }

  function buildTriggeringMessage(timestamp) {
    return {
      topic: 'ISSUE_OPENED',
      sender: 'system',
      timestamp,
    };
  }

  it('selects latest messages in chronological order', () => {
    const baseTime = Date.now();
    publishMessage('TEST_TOPIC', 'first-message', baseTime);
    publishMessage('TEST_TOPIC', 'second-message', baseTime + 10);
    publishMessage('TEST_TOPIC', 'third-message', baseTime + 20);

    const worker = createWorker({
      sources: [{ topic: 'TEST_TOPIC', amount: 2, strategy: 'latest' }],
    });

    const context = worker._buildContext(buildTriggeringMessage(baseTime + 30));

    assert(!context.includes('first-message'), 'Should not include oldest message');
    assert(context.includes('second-message'), 'Should include second message');
    assert(context.includes('third-message'), 'Should include latest message');
    assert(
      context.indexOf('second-message') < context.indexOf('third-message'),
      'Latest messages should render in chronological order'
    );
  });

  it('selects oldest messages in chronological order', () => {
    const baseTime = Date.now();
    publishMessage('TEST_TOPIC', 'alpha-message', baseTime);
    publishMessage('TEST_TOPIC', 'beta-message', baseTime + 10);
    publishMessage('TEST_TOPIC', 'gamma-message', baseTime + 20);

    const worker = createWorker({
      sources: [{ topic: 'TEST_TOPIC', amount: 2, strategy: 'oldest' }],
    });

    const context = worker._buildContext(buildTriggeringMessage(baseTime + 30));

    assert(context.includes('alpha-message'), 'Should include oldest message');
    assert(context.includes('beta-message'), 'Should include second message');
    assert(!context.includes('gamma-message'), 'Should not include latest message');
    assert(
      context.indexOf('alpha-message') < context.indexOf('beta-message'),
      'Oldest messages should render in chronological order'
    );
  });

  it('selects all messages in chronological order', () => {
    const baseTime = Date.now();
    publishMessage('TEST_TOPIC', 'one-message', baseTime);
    publishMessage('TEST_TOPIC', 'two-message', baseTime + 10);
    publishMessage('TEST_TOPIC', 'three-message', baseTime + 20);

    const worker = createWorker({
      sources: [{ topic: 'TEST_TOPIC', strategy: 'all' }],
    });

    const context = worker._buildContext(buildTriggeringMessage(baseTime + 30));

    assert(context.includes('one-message'), 'Should include first message');
    assert(context.includes('two-message'), 'Should include second message');
    assert(context.includes('three-message'), 'Should include third message');
    assert(
      context.indexOf('one-message') < context.indexOf('two-message'),
      'All messages should render in chronological order'
    );
    assert(
      context.indexOf('two-message') < context.indexOf('three-message'),
      'All messages should render in chronological order'
    );
  });

  it('uses limit as amount alias with latest default', () => {
    const baseTime = Date.now();
    publishMessage('TEST_TOPIC', 'old-message', baseTime);
    publishMessage('TEST_TOPIC', 'newer-message', baseTime + 10);

    const worker = createWorker({
      sources: [{ topic: 'TEST_TOPIC', limit: 1 }],
    });

    const context = worker._buildContext(buildTriggeringMessage(baseTime + 20));

    assert(!context.includes('old-message'), 'Should not include older message');
    assert(context.includes('newer-message'), 'Should include latest message');
  });

  it('prefers amount when both amount and limit are set', () => {
    const baseTime = Date.now();
    publishMessage('TEST_TOPIC', 'older-message', baseTime);
    publishMessage('TEST_TOPIC', 'newest-message', baseTime + 10);

    const worker = createWorker({
      sources: [{ topic: 'TEST_TOPIC', amount: 1, limit: 2, strategy: 'latest' }],
    });

    const context = worker._buildContext(buildTriggeringMessage(baseTime + 20));

    assert(!context.includes('older-message'), 'Should honor amount over limit');
    assert(context.includes('newest-message'), 'Should include latest message');
  });
});
