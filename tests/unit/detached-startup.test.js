const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawn } = require('child_process');

const {
  getClustersFilePath,
  isClusterRegistered,
  resolveWaitTimeoutMs,
  waitForClusterRegistration,
} = require('../../lib/detached-startup');

describe('detached-startup helpers', function () {
  let tempRoot = null;
  /** @type {NodeJS.Timeout | null} */
  let pendingTimer = null;

  function createTempStorageDir() {
    tempRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'zs-detached-startup-'));
    return tempRoot;
  }

  afterEach(function () {
    if (pendingTimer) {
      clearTimeout(pendingTimer);
      pendingTimer = null;
    }
    if (tempRoot) {
      fs.rmSync(tempRoot, { recursive: true, force: true });
      tempRoot = null;
    }
  });

  it('uses default timeout when value is invalid', function () {
    assert.strictEqual(resolveWaitTimeoutMs(undefined), 180000);
    assert.strictEqual(resolveWaitTimeoutMs(0), 180000);
    assert.strictEqual(resolveWaitTimeoutMs(-10), 180000);
    assert.strictEqual(resolveWaitTimeoutMs('bad-value'), 180000);
  });

  it('converts valid wait timeout to milliseconds', function () {
    assert.strictEqual(resolveWaitTimeoutMs(2), 2000);
    assert.strictEqual(resolveWaitTimeoutMs('2.5'), 2500);
  });

  it('returns false when clusters.json does not exist', function () {
    const storageDir = createTempStorageDir();
    assert.strictEqual(isClusterRegistered('missing-cluster', storageDir), false);
  });

  it('returns true when cluster is present in clusters.json', function () {
    const storageDir = createTempStorageDir();
    const clustersFile = getClustersFilePath(storageDir);
    fs.mkdirSync(path.dirname(clustersFile), { recursive: true });
    fs.writeFileSync(
      clustersFile,
      JSON.stringify({
        'ready-cluster': { id: 'ready-cluster' },
      })
    );

    assert.strictEqual(isClusterRegistered('ready-cluster', storageDir), true);
  });

  it('waits until cluster is registered', async function () {
    this.timeout(5000);
    const storageDir = createTempStorageDir();
    const clustersFile = getClustersFilePath(storageDir);
    const clusterId = 'cluster-waits';

    fs.mkdirSync(path.dirname(clustersFile), { recursive: true });
    fs.writeFileSync(clustersFile, JSON.stringify({}));

    pendingTimer = setTimeout(() => {
      fs.writeFileSync(
        clustersFile,
        JSON.stringify({
          [clusterId]: { id: clusterId },
        })
      );
    }, 80);

    const result = await waitForClusterRegistration({
      clusterId,
      timeoutMs: 2000,
      pollMs: 20,
      storageDir,
    });

    assert.strictEqual(result.ready, true);
    assert(result.elapsedMs >= 0);
  });

  it('fails on timeout when cluster never appears', async function () {
    const storageDir = createTempStorageDir();
    const clustersFile = getClustersFilePath(storageDir);
    fs.mkdirSync(path.dirname(clustersFile), { recursive: true });
    fs.writeFileSync(clustersFile, JSON.stringify({}));

    await assert.rejects(
      waitForClusterRegistration({
        clusterId: 'never-there',
        timeoutMs: 120,
        pollMs: 20,
        storageDir,
      }),
      /Timed out/
    );
  });

  it('fails fast when daemon exits before registration', async function () {
    this.timeout(5000);
    const storageDir = createTempStorageDir();
    const clustersFile = getClustersFilePath(storageDir);
    fs.mkdirSync(path.dirname(clustersFile), { recursive: true });
    fs.writeFileSync(clustersFile, JSON.stringify({}));

    const child = spawn(process.execPath, ['-e', 'process.exit(0)']);
    await new Promise((resolve) => child.on('exit', resolve));

    await assert.rejects(
      waitForClusterRegistration({
        clusterId: 'daemon-died',
        timeoutMs: 1000,
        pollMs: 20,
        storageDir,
        daemonPid: child.pid,
      }),
      /exited before cluster/
    );
  });
});
