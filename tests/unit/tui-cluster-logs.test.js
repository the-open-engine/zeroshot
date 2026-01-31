const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { execSync } = require('child_process');
const { loadTuiModule } = require('../helpers/load-tui');

const buildOutput = path.join(__dirname, '..', '..', 'lib', 'tui', 'services', 'cluster-logs.js');

function ensureTuiBuild() {
  if (!fs.existsSync(buildOutput)) {
    execSync('npm run build:tui', { stdio: 'inherit' });
  }
}

ensureTuiBuild();

let createClusterLogStream;
let resolveClusterDbPath;

before(async function () {
  ({ createClusterLogStream, resolveClusterDbPath } = await loadTuiModule(
    'lib/tui/services/cluster-logs.js'
  ));
});
const Ledger = require('../../src/ledger');

describe('TUI cluster logs service', function () {
  const originalHome = process.env.HOME;
  let tempHome;

  beforeEach(function () {
    tempHome = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-tui-logs-'));
    process.env.HOME = tempHome;
  });

  afterEach(function () {
    process.env.HOME = originalHome;
    fs.rmSync(tempHome, { recursive: true, force: true });
  });

  it('resolves dbPath from clusters.json when available', function () {
    const clusterId = 'cluster-test-1';
    const zeroshotDir = path.join(tempHome, '.zeroshot');
    fs.mkdirSync(zeroshotDir, { recursive: true });

    const dbPath = path.join(zeroshotDir, 'custom.db');
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

    assert.strictEqual(resolveClusterDbPath(clusterId), dbPath);
  });

  it('streams logs once the ledger appears', async function () {
    const clusterId = 'cluster-test-2';
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

    const seen = [];
    const statuses = [];

    const stream = createClusterLogStream({
      clusterId,
      pollIntervalMs: 25,
      onLines: (lines) => {
        seen.push(...lines);
      },
      onStatus: (status) => statuses.push(status),
    });

    stream.start();

    await new Promise((resolve) => setTimeout(resolve, 60));

    const writer = new Ledger(dbPath);
    writer.append({
      cluster_id: clusterId,
      topic: 'AGENT_OUTPUT',
      sender: 'worker',
      content: {
        text: 'first log',
        data: {
          agent: 'worker',
          role: 'implementation',
          line: 'first log',
        },
      },
    });
    writer.close();

    await new Promise((resolve) => setTimeout(resolve, 80));

    const writer2 = new Ledger(dbPath);
    writer2.append({
      cluster_id: clusterId,
      topic: 'AGENT_OUTPUT',
      sender: 'worker',
      content: {
        text: 'second log',
        data: {
          agent: 'worker',
          role: 'implementation',
          line: 'second log',
        },
      },
    });
    writer2.close();

    await new Promise((resolve) => setTimeout(resolve, 80));

    stream.close();

    assert.ok(statuses.some((status) => status.state === 'waiting'));
    assert.ok(statuses.some((status) => status.state === 'ready'));
    assert.ok(seen.some((line) => line.text.includes('first log')));
    assert.ok(seen.some((line) => line.text.includes('second log')));
  });

  it('filters log lines by agent id', async function () {
    const clusterId = 'cluster-test-3';
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

    const writer = new Ledger(dbPath);
    writer.append({
      cluster_id: clusterId,
      topic: 'AGENT_OUTPUT',
      sender: 'alpha',
      content: {
        text: 'alpha one',
        data: {
          agent: 'alpha',
          role: 'implementation',
          line: 'alpha one',
        },
      },
    });
    writer.append({
      cluster_id: clusterId,
      topic: 'AGENT_OUTPUT',
      sender: 'bravo',
      content: {
        text: 'bravo one',
        data: {
          agent: 'bravo',
          role: 'review',
          line: 'bravo one',
        },
      },
    });
    writer.close();

    const seen = [];

    const stream = createClusterLogStream({
      clusterId,
      agentId: 'alpha',
      pollIntervalMs: 25,
      onLines: (lines) => {
        seen.push(...lines);
      },
    });

    stream.start();

    await new Promise((resolve) => setTimeout(resolve, 80));

    const writer2 = new Ledger(dbPath);
    writer2.append({
      cluster_id: clusterId,
      topic: 'AGENT_OUTPUT',
      sender: 'bravo',
      content: {
        text: 'bravo two',
        data: {
          agent: 'bravo',
          role: 'review',
          line: 'bravo two',
        },
      },
    });
    writer2.append({
      cluster_id: clusterId,
      topic: 'AGENT_OUTPUT',
      sender: 'alpha',
      content: {
        text: 'alpha two',
        data: {
          agent: 'alpha',
          role: 'implementation',
          line: 'alpha two',
        },
      },
    });
    writer2.close();

    await new Promise((resolve) => setTimeout(resolve, 80));

    stream.close();

    assert.ok(seen.some((line) => line.text.includes('alpha one')));
    assert.ok(seen.some((line) => line.text.includes('alpha two')));
    assert.ok(!seen.some((line) => line.text.includes('bravo')));
  });
});
