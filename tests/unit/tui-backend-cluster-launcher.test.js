const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const PROJECT_ROOT = path.resolve(__dirname, '../..');
const LAUNCHER_PATH = path.join(
  PROJECT_ROOT,
  'lib',
  'tui-backend',
  'services',
  'cluster-launcher.js'
);

const loadLauncher = () => {
  if (!fs.existsSync(LAUNCHER_PATH)) {
    execSync('npm run build:tui-backend', { cwd: PROJECT_ROOT, stdio: 'inherit' });
  }
  return require(LAUNCHER_PATH);
};

describe('tui-backend cluster launcher', function () {
  let launchClusterFromIssue;
  let InvalidIssueReferenceError;

  before(function () {
    ({ launchClusterFromIssue, InvalidIssueReferenceError } = loadLauncher());
  });

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
