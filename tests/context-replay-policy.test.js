const assert = require('assert');
const AgentWrapper = require('../src/agent-wrapper');
const Ledger = require('../src/ledger');
const MessageBus = require('../src/message-bus');
const {
  RAW_LOG_ONLY_REPLAY_POLICY,
  buildRawLogOnlyMetadata,
  isReplayableMessage,
} = require('../src/agent/context-replay-policy');
const { broadcastAgentLine, broadcastIsolatedLine } = require('../src/agent/agent-task-executor');
const fs = require('fs');
const os = require('os');
const path = require('path');

describe('context replay policy', () => {
  it('treats raw provider output as raw-log-only unless explicitly context-safe', () => {
    assert.strictEqual(isReplayableMessage({ topic: 'AGENT_OUTPUT' }), false);
    assert.strictEqual(
      isReplayableMessage({
        topic: 'AGENT_OUTPUT',
        metadata: buildRawLogOnlyMetadata(),
      }),
      false
    );
    assert.strictEqual(
      isReplayableMessage({
        topic: 'AGENT_OUTPUT',
        metadata: { contextSafe: true },
      }),
      true
    );
    assert.strictEqual(
      isReplayableMessage({
        topic: 'VALIDATION_RESULT',
        content: { text: 'compact status' },
      }),
      true
    );
  });

  it('applies the same raw-log-only metadata to normal and isolated provider output', () => {
    const normalMessages = [];
    const agent = {
      id: 'worker',
      role: 'implementation',
      iteration: 2,
      lastOutputTime: 0,
      _publish: (message) => normalMessages.push(message),
    };
    const state = { output: '' };

    broadcastAgentLine({
      agent,
      providerName: 'codex',
      state,
      line: '[1700000000000]{"type":"compiler-artifact"}',
    });

    const isolatedMessages = [];
    broadcastIsolatedLine({
      agent: {
        id: 'isolated-worker',
        iteration: 3,
        cluster: { id: 'cluster-1' },
        messageBus: { publish: (message) => isolatedMessages.push(message) },
        lastOutputTime: 0,
      },
      providerName: 'codex',
      taskId: 'task-1',
      line: '[2026-05-06T12:00:00.000Z] *** Begin Patch',
    });

    assert.deepStrictEqual(normalMessages[0].metadata, {
      contextSafe: false,
      replayPolicy: RAW_LOG_ONLY_REPLAY_POLICY,
    });
    assert.deepStrictEqual(isolatedMessages[0].metadata, {
      contextSafe: false,
      replayPolicy: RAW_LOG_ONLY_REPLAY_POLICY,
    });
  });
});

describe('context replay with persisted messages', () => {
  let tempDir;
  let dbPath;
  let ledger;
  let messageBus;
  const clusterId = 'context-replay-cluster';
  const clusterCreatedAt = 1700000000000;

  beforeEach(() => {
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-context-replay-'));
    dbPath = path.join(tempDir, 'ledger.db');
    ledger = new Ledger(dbPath);
    messageBus = new MessageBus(ledger);
  });

  afterEach(() => {
    if (ledger) ledger.close();
    fs.rmSync(tempDir, { recursive: true, force: true });
  });

  function createWorker(bus) {
    return new AgentWrapper(
      {
        id: 'worker',
        role: 'implementation',
        modelLevel: 'level2',
        timeout: 0,
        contextStrategy: {
          sources: [{ topic: 'AGENT_OUTPUT', strategy: 'all' }],
        },
      },
      bus,
      {
        id: clusterId,
        createdAt: clusterCreatedAt,
        agents: [],
      },
      {
        testMode: true,
        mockSpawnFn: () => {},
      }
    );
  }

  function publish(message) {
    messageBus.publish({
      cluster_id: clusterId,
      sender: 'worker',
      timestamp: clusterCreatedAt + 10,
      ...message,
    });
  }

  it('keeps raw provider output in the ledger while excluding it from replay after reload', () => {
    publish({
      topic: 'AGENT_OUTPUT',
      content: {
        text: '{"type":"compiler-artifact","aggregated_output":"raw command transcript"}',
        data: {
          line: [
            '{"type":"compiler-artifact"}',
            '*** Begin Patch',
            '"aggregated_output":"raw command transcript"',
          ].join('\n'),
        },
      },
      metadata: buildRawLogOnlyMetadata(),
    });
    publish({
      topic: 'AGENT_OUTPUT',
      content: {
        text: 'compact validation status: fix the reported syntax error',
        data: { contextSafe: true },
      },
      metadata: { contextSafe: true },
    });

    const storedBeforeReload = messageBus.query({
      cluster_id: clusterId,
      topic: 'AGENT_OUTPUT',
    });
    assert(
      storedBeforeReload[0].content.text.includes('compiler-artifact'),
      'raw provider output should remain stored before reload'
    );

    ledger.close();
    ledger = new Ledger(dbPath);
    const reloadedBus = new MessageBus(ledger);
    const storedAfterReload = reloadedBus.query({
      cluster_id: clusterId,
      topic: 'AGENT_OUTPUT',
    });
    assert(
      storedAfterReload[0].content.data.line.includes('*** Begin Patch'),
      'raw provider output should remain stored after reload'
    );

    const context = createWorker(reloadedBus)._buildContext({
      topic: 'ISSUE_OPENED',
      sender: 'system',
      timestamp: clusterCreatedAt + 100,
      content: { text: 'trigger' },
    });

    assert(!context.includes('compiler-artifact'), 'raw compiler artifacts must not replay');
    assert(!context.includes('*** Begin Patch'), 'raw patch bodies must not replay');
    assert(!context.includes('aggregated_output'), 'raw command transcripts must not replay');
    assert(
      context.includes('compact validation status: fix the reported syntax error'),
      'explicit context-safe status should replay'
    );
  });

  it('does not let unmarked raw AGENT_OUTPUT consume latest replay slots', () => {
    publish({
      topic: 'AGENT_OUTPUT',
      timestamp: clusterCreatedAt + 10,
      content: { text: 'safe older status', data: { contextSafe: true } },
      metadata: { contextSafe: true },
    });
    publish({
      topic: 'AGENT_OUTPUT',
      timestamp: clusterCreatedAt + 20,
      content: { text: '{"type":"compiler-artifact"}' },
    });

    const worker = new AgentWrapper(
      {
        id: 'worker',
        role: 'implementation',
        modelLevel: 'level2',
        timeout: 0,
        contextStrategy: {
          sources: [{ topic: 'AGENT_OUTPUT', amount: 1, strategy: 'latest' }],
        },
      },
      messageBus,
      {
        id: clusterId,
        createdAt: clusterCreatedAt,
        agents: [],
      },
      {
        testMode: true,
        mockSpawnFn: () => {},
      }
    );

    const context = worker._buildContext({
      topic: 'ISSUE_OPENED',
      sender: 'system',
      timestamp: clusterCreatedAt + 100,
      content: { text: 'trigger' },
    });

    assert(context.includes('safe older status'), 'latest source should select replayable rows');
    assert(!context.includes('compiler-artifact'), 'unmarked AGENT_OUTPUT defaults raw-log-only');
  });
});
