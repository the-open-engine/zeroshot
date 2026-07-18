const assert = require('assert');
const fs = require('fs');
const path = require('path');

const socketPaths = require('../../src/attach/socket-paths');

describe('attach socket paths', function () {
  const createdDirs = new Set();

  afterEach(() => {
    for (const socketDir of createdDirs) {
      fs.rmSync(socketDir, { recursive: true, force: true });
    }
    createdDirs.clear();
  });

  function taskPath(taskId, homeDir) {
    const socketPath = socketPaths.getTaskSocketPath(taskId, homeDir);
    createdDirs.add(path.dirname(socketPath));
    return socketPath;
  }

  it('allocates deterministic short paths isolated by Zeroshot home', function () {
    const longHome = path.join('/tmp', 'long-home-segment-'.repeat(20));
    const first = taskPath('sparkling-fortress-99', longHome);
    const repeated = taskPath('sparkling-fortress-99', longHome);
    const otherHome = taskPath('sparkling-fortress-99', `${longHome}-other`);

    assert.strictEqual(first, repeated);
    assert.notStrictEqual(first, otherHome);
    assert(Buffer.byteLength(first) < 100, `socket path is too long: ${first}`);
    assert(!first.includes(longHome));
  });

  it('protects each per-user socket directory with owner-only permissions', function () {
    if (process.platform === 'win32') this.skip();

    const socketPath = taskPath('secure-task', '/tmp/zeroshot-secure-home');
    const mode = fs.statSync(path.dirname(socketPath)).mode & 0o777;

    assert.strictEqual(mode, 0o700);
  });
});
