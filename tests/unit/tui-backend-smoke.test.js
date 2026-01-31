const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const buildOutput = path.join(
  __dirname,
  '..',
  '..',
  'lib',
  'tui-backend',
  'services',
  'cluster-registry.js'
);
const sourcePath = path.join(
  __dirname,
  '..',
  '..',
  'src',
  'tui-backend',
  'services',
  'cluster-registry.ts'
);

function ensureBackendBuild() {
  if (!fs.existsSync(buildOutput)) {
    execSync('npm run build:tui-backend', { stdio: 'inherit' });
    return;
  }
  if (fs.existsSync(sourcePath)) {
    const buildMtime = fs.statSync(buildOutput).mtimeMs;
    const sourceMtime = fs.statSync(sourcePath).mtimeMs;
    if (sourceMtime > buildMtime) {
      execSync('npm run build:tui-backend', { stdio: 'inherit' });
    }
  }
}

ensureBackendBuild();

describe('TUI backend build', function () {
  it('exposes cluster registry services', function () {
    const registry = require('../../lib/tui-backend/services/cluster-registry');
    assert.ok(registry);
    assert.strictEqual(typeof registry.listClusters, 'function');
    assert.strictEqual(typeof registry.getClusterSummary, 'function');
    assert.strictEqual(typeof registry.listClusterMetrics, 'function');
  });
});
