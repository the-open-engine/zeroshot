const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { execSync, spawn } = require('child_process');

const PROJECT_ROOT = path.resolve(__dirname, '../..');
const SERVER_PATH = path.join(PROJECT_ROOT, 'lib', 'tui-backend', 'server.js');
const SERVER_SOURCE_PATH = path.join(PROJECT_ROOT, 'src', 'tui-backend', 'server.ts');

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

  before(function () {
    if (isBuildStale(SERVER_SOURCE_PATH, SERVER_PATH)) {
      execSync('npm run build:tui-backend', { cwd: PROJECT_ROOT, stdio: 'inherit' });
    }

    server = spawn('node', [SERVER_PATH], {
      cwd: PROJECT_ROOT,
      stdio: ['pipe', 'pipe', 'pipe'],
      env: { ...process.env, ZEROSHOT_TUI_BACKEND_MOCK_LAUNCH: '1' },
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
    assert.deepStrictEqual(response.result.capabilities.notifications, []);
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
});
