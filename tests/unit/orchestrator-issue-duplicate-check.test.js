const assert = require('node:assert');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const Orchestrator = require('../../src/orchestrator');

describe('Orchestrator duplicate issue check', function () {
  this.timeout(5000);

  let tempDir;
  let orchestrator;

  beforeEach(() => {
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-orchestrator-dup-'));
    orchestrator = new Orchestrator({ quiet: true, skipLoad: true, storageDir: tempDir });
  });

  afterEach(() => {
    if (tempDir && fs.existsSync(tempDir)) {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  it('does not treat the current cluster as a duplicate of itself', () => {
    const clusterId = 'self';
    orchestrator.clusters.set(clusterId, {
      id: clusterId,
      issue: 1172,
      state: 'initializing',
      pid: process.pid,
      createdAt: Date.now(),
    });

    const active = orchestrator._getActiveClustersForIssue(1172, clusterId);
    assert.deepEqual(active, []);
  });
});
