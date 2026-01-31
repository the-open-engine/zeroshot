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
  'cluster-launcher.js'
);
const sourcePath = path.join(
  __dirname,
  '..',
  '..',
  'src',
  'tui-backend',
  'services',
  'cluster-launcher.ts'
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

const {
  launchClusterFromIssue,
  InvalidIssueReferenceError,
} = require('../../lib/tui-backend/services/cluster-launcher');

describe('tui-backend cluster launcher', function () {
  it('throws InvalidIssueReferenceError for invalid issue refs', async function () {
    await assert.rejects(
      () =>
        launchClusterFromIssue({
          ref: 'not-an-issue',
          deps: {
            detectRunInput: () => ({ text: 'not-an-issue' }),
          },
        }),
      (error) => {
        assert.ok(error instanceof InvalidIssueReferenceError);
        assert.ok(error.message.includes('Invalid issue reference: not-an-issue'));
        return true;
      }
    );
  });

  it('forwards providerOverride and clusterId to startClusterFromIssue', async function () {
    const calls = [];
    const deps = {
      getOrchestrator: () => ({ id: 'orch' }),
      loadSettings: () => ({ defaultConfig: 'conductor-bootstrap', providerSettings: {} }),
      resolveConfigPath: () => '/tmp/config.json',
      loadClusterConfig: () => ({ name: 'config' }),
      detectRunInput: () => ({ issue: '123' }),
      startClusterFromIssue: (args) => {
        calls.push(args);
      },
      generateClusterId: () => 'generated',
    };

    const result = await launchClusterFromIssue({
      ref: '123',
      providerOverride: 'codex',
      clusterId: 'cluster-789',
      deps,
    });

    assert.deepStrictEqual(result, { clusterId: 'cluster-789' });
    assert.strictEqual(calls.length, 1);
    assert.strictEqual(calls[0].issue, '123');
    assert.strictEqual(calls[0].providerOverride, 'codex');
    assert.strictEqual(calls[0].clusterId, 'cluster-789');
  });
});
