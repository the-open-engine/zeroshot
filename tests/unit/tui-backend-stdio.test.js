const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { execSync, spawn } = require('child_process');
const Ledger = require('../../src/ledger');

const PROJECT_ROOT = path.resolve(__dirname, '../..');
const SERVER_PATH = path.join(PROJECT_ROOT, 'lib', 'tui-backend', 'server.js');
const SERVER_SOURCE_PATH = path.join(PROJECT_ROOT, 'src', 'tui-backend', 'server.ts');
const CLUSTER_LOGS_SOURCE_PATH = path.join(
  PROJECT_ROOT,
  'src',
  'tui-backend',
  'services',
  'cluster-logs.ts'
);
const CLUSTER_LOGS_BUILD_PATH = path.join(
  PROJECT_ROOT,
  'lib',
  'tui-backend',
  'services',
  'cluster-logs.js'
);

function isBuildStale(sourcePath, buildPath) {
  if (!fs.existsSync(buildPath)) {
    return true;
  }
  if (!fs.existsSync(sourcePath)) {
    return false;
  }
  return fs.statSync(sourcePath).mtimeMs > fs.statSync(buildPath).mtimeMs;
}

const encodeFrame = (payload) => {
  const body = Buffer.from(typeof payload === 'string' ? payload : JSON.stringify(payload), 'utf8');
  const header = Buffer.from(`Content-Length: ${body.length}\r\n\r\n`, 'utf8');
  return Buffer.concat([header, body]);
};

const createFrameCollector = () => {
  let buffer = Buffer.alloc(0);
  return (chunk) => {
    buffer = Buffer.concat([buffer, chunk]);
    const frames = [];
    while (true) {
      const headerIndex = buffer.indexOf('\r\n\r\n');
      if (headerIndex === -1) {
        break;
      }
      const headerText = buffer.slice(0, headerIndex).toString('utf8');
      const match = headerText.match(/Content-Length:\s*(\d+)/i);
      if (!match) {
        throw new Error('Missing Content-Length header in response');
      }
      const length = Number.parseInt(match[1], 10);
      const totalLength = headerIndex + 4 + length;
      if (buffer.length < totalLength) {
        break;
      }
      const payload = buffer.slice(headerIndex + 4, totalLength).toString('utf8');
      frames.push(payload);
      buffer = buffer.slice(totalLength);
    }
    return frames;
  };
};

const createMessageQueue = () => {
  const queue = [];
  const waiters = [];
  return {
    push(message) {
      if (waiters.length) {
        const waiter = waiters.shift();
        clearTimeout(waiter.timer);
        waiter.resolve(message);
        return;
      }
      queue.push(message);
    },
    next(timeoutMs = 2000) {
      if (queue.length) {
        return Promise.resolve(queue.shift());
      }
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          const index = waiters.findIndex((waiter) => waiter.resolve === resolve);
          if (index !== -1) {
            waiters.splice(index, 1);
          }
          reject(new Error('Timed out waiting for response'));
        }, timeoutMs);
        waiters.push({ resolve, reject, timer });
      });
    },
  };
};

describe('tui-backend stdio JSON-RPC', function () {
  this.timeout(15000);

  let server;
  let queue;
  const originalHome = process.env.HOME;
  let tempHome;
  let MAX_LOG_LINES;
  const topologyClusterId = 'cluster-stdio-topology';
  const metricsClusterId = 'cluster-stdio-metrics';

  const waitForMessage = async (predicate, timeoutMs = 2000) => {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const remaining = Math.max(deadline - Date.now(), 10);
      try {
        const message = await queue.next(remaining);
        if (predicate(message)) {
          return message;
        }
      } catch (error) {
        if (Date.now() >= deadline) {
          throw error;
        }
      }
    }
    throw new Error('Timed out waiting for matching response');
  };

  const expectNoMessage = async (predicate, timeoutMs = 300) => {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const remaining = Math.max(deadline - Date.now(), 10);
      try {
        const message = await queue.next(remaining);
        if (predicate(message)) {
          assert.fail(`Unexpected message: ${JSON.stringify(message)}`);
        }
      } catch (error) {
        if (error && String(error.message).includes('Timed out waiting for response')) {
          return;
        }
        throw error;
      }
    }
  };

  before(function () {
    tempHome = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-tui-backend-'));
    process.env.HOME = tempHome;
    const zeroshotDir = path.join(tempHome, '.zeroshot');
    fs.mkdirSync(zeroshotDir, { recursive: true });

    const clustersFile = path.join(zeroshotDir, 'clusters.json');
    const now = Date.now();
    const baseConfig = {
      agents: [
        {
          id: 'worker',
          role: 'implementation',
          triggers: [{ topic: 'ISSUE_OPENED', action: 'execute_task' }],
          hooks: { onComplete: { config: { topic: 'IMPLEMENTATION_READY' } } },
        },
        {
          id: 'validator',
          role: 'validator',
          triggers: [{ topic: 'IMPLEMENTATION_READY', action: 'execute_task' }],
        },
      ],
    };
    const clustersData = {
      [topologyClusterId]: {
        id: topologyClusterId,
        config: baseConfig,
        state: 'stopped',
        createdAt: now - 1000,
        pid: null,
      },
      [metricsClusterId]: {
        id: metricsClusterId,
        config: baseConfig,
        state: 'stopped',
        createdAt: now,
        pid: null,
      },
    };
    fs.writeFileSync(clustersFile, JSON.stringify(clustersData, null, 2));

    for (const clusterId of [topologyClusterId, metricsClusterId]) {
      const dbPath = path.join(zeroshotDir, `${clusterId}.db`);
      const ledger = new Ledger(dbPath);
      ledger.append({
        cluster_id: clusterId,
        topic: 'SYSTEM',
        sender: 'test',
        content: { text: `seed ${clusterId}`, data: { line: `seed ${clusterId}` } },
      });
      ledger.close();
    }

    if (
      isBuildStale(SERVER_SOURCE_PATH, SERVER_PATH) ||
      isBuildStale(CLUSTER_LOGS_SOURCE_PATH, CLUSTER_LOGS_BUILD_PATH)
    ) {
      execSync('npm run build:tui-backend', { cwd: PROJECT_ROOT, stdio: 'inherit' });
    }

    ({ MAX_LOG_LINES } = require('../../lib/tui-backend/services/cluster-logs'));

    server = spawn('node', [SERVER_PATH], {
      cwd: PROJECT_ROOT,
      stdio: ['pipe', 'pipe', 'pipe'],
      env: {
        ...process.env,
        ZEROSHOT_TUI_BACKEND_MOCK_LAUNCH: '1',
        ZEROSHOT_TUI_BACKEND_MOCK_GUIDANCE: '1',
        ZEROSHOT_TUI_BACKEND_METRICS_PLATFORM: 'sunos',
        HOME: tempHome,
      },
    });

    queue = createMessageQueue();
    const collectFrames = createFrameCollector();

    server.stdout.on('data', (chunk) => {
      const frames = collectFrames(chunk);
      for (const frame of frames) {
        queue.push(JSON.parse(frame));
      }
    });

    server.stderr.on('data', () => {});
  });

  after(async function () {
    if (!server) return;
    server.stdin.end();
    await new Promise((resolve) => server.on('exit', resolve));
    process.env.HOME = originalHome;
    fs.rmSync(tempHome, { recursive: true, force: true });
  });

  it('responds to initialize with capabilities', async function () {
    const request = {
      jsonrpc: '2.0',
      id: 1,
      method: 'initialize',
      params: {
        protocolVersion: 1,
        client: { name: 'test-client', version: '0.1.0' },
      },
    };

    server.stdin.write(encodeFrame(request));
    const response = await queue.next();

    assert.strictEqual(response.id, 1);
    assert.strictEqual(response.jsonrpc, '2.0');
    assert.strictEqual(response.result.protocolVersion, 1);
    assert.ok(response.result.server.name);
    assert.ok(response.result.server.version);
    assert.ok(response.result.capabilities.methods.includes('initialize'));
    assert.ok(response.result.capabilities.methods.includes('ping'));
    assert.ok(response.result.capabilities.methods.includes('listClusters'));
    assert.ok(response.result.capabilities.methods.includes('getClusterSummary'));
    assert.ok(response.result.capabilities.methods.includes('listClusterMetrics'));
    assert.ok(response.result.capabilities.methods.includes('getClusterTopology'));
    assert.ok(response.result.capabilities.methods.includes('unsubscribe'));
    assert.deepStrictEqual(response.result.capabilities.notifications, [
      'clusterLogLines',
      'clusterTimelineEvents',
    ]);
  });

  it('responds to ping', async function () {
    const request = {
      jsonrpc: '2.0',
      id: 2,
      method: 'ping',
      params: {},
    };

    server.stdin.write(encodeFrame(request));
    const response = await queue.next();

    assert.strictEqual(response.id, 2);
    assert.deepStrictEqual(response.result, { ok: true });
  });

  it('responds to listClusters with provider and cwd fields', async function () {
    const request = {
      jsonrpc: '2.0',
      id: 7,
      method: 'listClusters',
      params: {},
    };

    server.stdin.write(encodeFrame(request));
    const response = await queue.next();

    assert.strictEqual(response.id, 7);
    assert.strictEqual(response.jsonrpc, '2.0');
    assert.ok(response.result);
    assert.ok(Array.isArray(response.result.clusters));
    for (const cluster of response.result.clusters) {
      assert.ok(Object.prototype.hasOwnProperty.call(cluster, 'provider'));
      assert.ok(Object.prototype.hasOwnProperty.call(cluster, 'cwd'));
    }
  });

  it('returns cluster not found for unknown cluster id', async function () {
    const request = {
      jsonrpc: '2.0',
      id: 8,
      method: 'getClusterSummary',
      params: { clusterId: 'missing-cluster-stdio' },
    };

    server.stdin.write(encodeFrame(request));
    const response = await queue.next();

    assert.strictEqual(response.id, 8);
    assert.strictEqual(response.error.code, -32002);
  });

  it('responds to getClusterSummary for a known cluster when available', async function () {
    const listRequest = {
      jsonrpc: '2.0',
      id: 9,
      method: 'listClusters',
      params: {},
    };

    server.stdin.write(encodeFrame(listRequest));
    const listResponse = await queue.next();
    const clusters = listResponse.result?.clusters ?? [];
    if (clusters.length === 0) {
      return;
    }

    const request = {
      jsonrpc: '2.0',
      id: 10,
      method: 'getClusterSummary',
      params: { clusterId: clusters[0].id },
    };

    server.stdin.write(encodeFrame(request));
    const response = await queue.next();

    assert.strictEqual(response.id, 10);
    assert.strictEqual(response.result.summary.id, clusters[0].id);
    assert.ok(Object.prototype.hasOwnProperty.call(response.result.summary, 'provider'));
    assert.ok(Object.prototype.hasOwnProperty.call(response.result.summary, 'cwd'));
  });

  it('responds to getClusterTopology for a seeded cluster', async function () {
    const request = {
      jsonrpc: '2.0',
      id: 17,
      method: 'getClusterTopology',
      params: { clusterId: topologyClusterId },
    };

    server.stdin.write(encodeFrame(request));
    const response = await queue.next();

    assert.strictEqual(response.id, 17);
    assert.ok(response.result.topology);
    assert.ok(Array.isArray(response.result.topology.agents));
    assert.ok(Array.isArray(response.result.topology.edges));
    assert.ok(Array.isArray(response.result.topology.topics));
    assert.ok(response.result.topology.topics.includes('ISSUE_OPENED'));
    assert.ok(response.result.topology.topics.includes('IMPLEMENTATION_READY'));
    assert.ok(
      response.result.topology.edges.some(
        (edge) => edge.from === 'system' && edge.to === 'ISSUE_OPENED' && edge.kind === 'source'
      )
    );
    assert.ok(
      response.result.topology.edges.some(
        (edge) => edge.from === 'ISSUE_OPENED' && edge.to === 'worker'
      )
    );
    assert.ok(
      response.result.topology.edges.some(
        (edge) => edge.from === 'worker' && edge.to === 'IMPLEMENTATION_READY'
      )
    );
  });

  it('responds to listClusterMetrics with filtered cluster ids', async function () {
    const request = {
      jsonrpc: '2.0',
      id: 18,
      method: 'listClusterMetrics',
      params: {
        clusterIds: [metricsClusterId, 'missing-metrics', topologyClusterId],
      },
    };

    server.stdin.write(encodeFrame(request));
    const response = await queue.next();

    assert.strictEqual(response.id, 18);
    assert.ok(Array.isArray(response.result.metrics));
    assert.strictEqual(response.result.metrics.length, 2);
    assert.strictEqual(response.result.metrics[0].id, metricsClusterId);
    assert.strictEqual(response.result.metrics[1].id, topologyClusterId);
    for (const metric of response.result.metrics) {
      assert.strictEqual(metric.supported, false);
      assert.strictEqual(metric.cpuPercent, null);
      assert.strictEqual(metric.memoryMB, null);
    }
  });

  it('responds to startClusterFromText with a cluster id', async function () {
    const request = {
      jsonrpc: '2.0',
      id: 11,
      method: 'startClusterFromText',
      params: { text: 'Launch from text', clusterId: 'cluster-stdio' },
    };

    server.stdin.write(encodeFrame(request));
    const response = await queue.next();

    assert.strictEqual(response.id, 11);
    assert.deepStrictEqual(response.result, { clusterId: 'cluster-stdio' });
  });

  it('returns invalid params for invalid issue ref', async function () {
    const request = {
      jsonrpc: '2.0',
      id: 12,
      method: 'startClusterFromIssue',
      params: { ref: 'not-an-issue' },
    };

    server.stdin.write(encodeFrame(request));
    const response = await queue.next();

    assert.strictEqual(response.id, 12);
    assert.strictEqual(response.error.code, -32602);
    assert.ok(response.error.data.detail.includes('Invalid issue reference:'));
  });

  it('responds to sendGuidanceToAgent with delivery details', async function () {
    const request = {
      jsonrpc: '2.0',
      id: 13,
      method: 'sendGuidanceToAgent',
      params: {
        clusterId: 'cluster-guidance',
        agentId: 'agent-1',
        text: 'Use approach A',
        timeoutMs: 250,
      },
    };

    server.stdin.write(encodeFrame(request));
    const response = await queue.next();

    assert.strictEqual(response.id, 13);
    assert.strictEqual(response.result.result.status, 'injected');
    assert.strictEqual(response.result.result.reason, null);
    assert.strictEqual(response.result.result.method, 'pty');
    assert.strictEqual(response.result.result.taskId, 'task-agent-1');
  });

  it('responds to sendGuidanceToCluster with summary and agents', async function () {
    const request = {
      jsonrpc: '2.0',
      id: 14,
      method: 'sendGuidanceToCluster',
      params: {
        clusterId: 'cluster-guidance',
        text: 'Use approach B',
        timeoutMs: 500,
      },
    };

    server.stdin.write(encodeFrame(request));
    const response = await queue.next();

    assert.strictEqual(response.id, 14);
    assert.strictEqual(response.result.result.summary.total, 2);
    assert.strictEqual(response.result.result.summary.injected, 1);
    assert.strictEqual(response.result.result.summary.queued, 1);
    assert.strictEqual(response.result.result.agents['mock-agent-1'].status, 'injected');
    assert.strictEqual(response.result.result.agents['mock-agent-2'].status, 'queued');
    assert.strictEqual(response.result.result.timestamp, 1700000000000);
  });

  it('returns invalid params for empty agent guidance text', async function () {
    const request = {
      jsonrpc: '2.0',
      id: 15,
      method: 'sendGuidanceToAgent',
      params: {
        clusterId: 'cluster-guidance',
        agentId: 'agent-1',
        text: '   ',
      },
    };

    server.stdin.write(encodeFrame(request));
    const response = await queue.next();

    assert.strictEqual(response.id, 15);
    assert.strictEqual(response.error.code, -32602);
  });

  it('returns invalid params for empty cluster guidance text', async function () {
    const request = {
      jsonrpc: '2.0',
      id: 16,
      method: 'sendGuidanceToCluster',
      params: {
        clusterId: 'cluster-guidance',
        text: '',
      },
    };

    server.stdin.write(encodeFrame(request));
    const response = await queue.next();

    assert.strictEqual(response.id, 16);
    assert.strictEqual(response.error.code, -32602);
  });

  it('returns parse error for invalid JSON', async function () {
    server.stdin.write(encodeFrame('{ not-json'));
    const response = await queue.next();

    assert.strictEqual(response.id, null);
    assert.strictEqual(response.error.code, -32700);
  });

  it('returns method not found for unknown method', async function () {
    const request = {
      jsonrpc: '2.0',
      id: 3,
      method: 'nope',
      params: {},
    };

    server.stdin.write(encodeFrame(request));
    const response = await queue.next();

    assert.strictEqual(response.id, 3);
    assert.strictEqual(response.error.code, -32601);
  });

  it('returns invalid params for malformed initialize', async function () {
    const request = {
      jsonrpc: '2.0',
      id: 4,
      method: 'initialize',
      params: { protocolVersion: 1 },
    };

    server.stdin.write(encodeFrame(request));
    const response = await queue.next();

    assert.strictEqual(response.id, 4);
    assert.strictEqual(response.error.code, -32602);
  });

  it('returns protocol mismatch with supported versions', async function () {
    const request = {
      jsonrpc: '2.0',
      id: 5,
      method: 'initialize',
      params: {
        protocolVersion: 999,
        client: { name: 'test-client', version: '0.1.0' },
      },
    };

    server.stdin.write(encodeFrame(request));
    const response = await queue.next();

    assert.strictEqual(response.id, 5);
    assert.strictEqual(response.error.code, -32000);
    assert.deepStrictEqual(response.error.data.supportedVersions, [1]);
  });

  it('reassembles partial frames across chunks', async function () {
    const request = {
      jsonrpc: '2.0',
      id: 6,
      method: 'ping',
      params: {},
    };
    const frame = encodeFrame(request);
    const splitIndex = Math.floor(frame.length / 2);
    server.stdin.write(frame.slice(0, splitIndex));
    server.stdin.write(frame.slice(splitIndex));

    const response = await queue.next();
    assert.strictEqual(response.id, 6);
    assert.deepStrictEqual(response.result, { ok: true });
  });

  it('streams cluster logs and stops after unsubscribe', async function () {
    const clusterId = 'cluster-stdio-logs';
    const zeroshotDir = path.join(tempHome, '.zeroshot');
    fs.mkdirSync(zeroshotDir, { recursive: true });

    const dbPath = path.join(zeroshotDir, `${clusterId}.db`);
    const clustersFile = path.join(zeroshotDir, 'clusters.json');
    fs.writeFileSync(
      clustersFile,
      JSON.stringify(
        {
          [clusterId]: {
            config: {
              dbPath,
            },
          },
        },
        null,
        2
      )
    );

    const seedLedger = new Ledger(dbPath);
    seedLedger.close();

    const subscribeRequest = {
      jsonrpc: '2.0',
      id: 20,
      method: 'subscribeClusterLogs',
      params: { clusterId },
    };

    server.stdin.write(encodeFrame(subscribeRequest));
    const subscribeResponse = await waitForMessage((msg) => msg.id === 20);
    const subscriptionId = subscribeResponse.result.subscriptionId;

    const writer = new Ledger(dbPath);
    const payloadCount = MAX_LOG_LINES + 5;
    const messages = Array.from({ length: payloadCount }, (_, index) => ({
      cluster_id: clusterId,
      topic: 'AGENT_OUTPUT',
      sender: 'worker',
      content: {
        text: `line ${index}`,
        data: {
          agent: 'worker',
          role: 'implementation',
          line: `line ${index}`,
        },
      },
    }));
    writer.batchAppend(messages);
    writer.close();

    const notification = await waitForMessage(
      (msg) =>
        msg.method === 'clusterLogLines' &&
        msg.params &&
        msg.params.subscriptionId === subscriptionId,
      4000
    );

    assert.strictEqual(notification.params.clusterId, clusterId);
    assert.strictEqual(notification.params.lines.length, MAX_LOG_LINES);
    assert.strictEqual(notification.params.droppedCount, 5);

    const unsubscribeRequest = {
      jsonrpc: '2.0',
      id: 21,
      method: 'unsubscribe',
      params: { subscriptionId },
    };

    server.stdin.write(encodeFrame(unsubscribeRequest));
    const unsubscribeResponse = await waitForMessage((msg) => msg.id === 21);
    assert.deepStrictEqual(unsubscribeResponse.result, { removed: true });

    const writer2 = new Ledger(dbPath);
    writer2.append({
      cluster_id: clusterId,
      topic: 'AGENT_OUTPUT',
      sender: 'worker',
      content: {
        text: 'after unsubscribe',
        data: {
          agent: 'worker',
          role: 'implementation',
          line: 'after unsubscribe',
        },
      },
    });
    writer2.close();

    await expectNoMessage(
      (msg) =>
        msg.method === 'clusterLogLines' &&
        msg.params &&
        msg.params.subscriptionId === subscriptionId,
      600
    );
  });
});
