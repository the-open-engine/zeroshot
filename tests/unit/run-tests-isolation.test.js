const assert = require('node:assert');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const REPO_ROOT = path.join(__dirname, '..', '..');

describe('unit test runner isolation', function () {
  this.timeout(30000);

  it('ignores operator settings and ambient Zeroshot run state', function () {
    const operatorHome = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-operator-home-'));
    const settingsFile = path.join(operatorHome, '.zeroshot', 'settings.json');
    fs.mkdirSync(path.dirname(settingsFile), { recursive: true });
    fs.writeFileSync(
      settingsFile,
      JSON.stringify({
        defaultProvider: 'codex',
        defaultDelivery: 'ship',
        providerSettings: {
          codex: {
            levelOverrides: {
              level1: { model: 'not-a-test-model' },
              level2: { model: 'not-a-test-model' },
              level3: { model: 'not-a-test-model' },
            },
          },
        },
      }),
      'utf8'
    );

    try {
      const result = spawnSync(
        process.execPath,
        ['tests/run-tests.js', 'tests/unit/pr-mode-cluster-validation.test.js'],
        {
          cwd: REPO_ROOT,
          env: {
            ...process.env,
            HOME: operatorHome,
            USERPROFILE: operatorHome,
            ZEROSHOT_SETTINGS_FILE: settingsFile,
            ZEROSHOT_MODEL: 'not-a-test-model',
            ZEROSHOT_PROVIDER: 'codex',
            ZEROSHOT_RUN_OPTIONS: JSON.stringify({
              ship: true,
              pr: true,
              worktree: true,
              autoMerge: true,
            }),
            ZEROSHOT_PR: '1',
            ZEROSHOT_WORKTREE: '1',
            ZEROSHOT_DOCKER: '1',
            ZEROSHOT_CLOSE_ISSUE: 'always',
            ZEROSHOT_DAEMON: '1',
            ZEROSHOT_MERGE_QUEUE: '1',
          },
          encoding: 'utf8',
          timeout: 20000,
        }
      );

      assert.strictEqual(
        result.status,
        0,
        `runner inherited operator state:\n${result.stdout}\n${result.stderr}`
      );
    } finally {
      fs.rmSync(operatorHome, { recursive: true, force: true });
    }
  });
});
