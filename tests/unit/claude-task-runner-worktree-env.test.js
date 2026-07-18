const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');

const ClaudeTaskRunner = require('../../src/claude-task-runner');

describe('ClaudeTaskRunner worktree env forwarding', function () {
  /** @type {string[]} */
  let tempDirs = [];

  afterEach(function () {
    for (const dir of tempDirs.splice(0)) {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  it('prepends worktree-local tool bins when cwd is inside a nested submodule', function () {
    const worktreeRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-runner-worktree-'));
    tempDirs.push(worktreeRoot);

    const toolBinDir = path.join(worktreeRoot, '.zeroshot', 'bin');
    const submoduleCwd = path.join(worktreeRoot, 'external', 'zeroshot', 'src');
    fs.mkdirSync(toolBinDir, { recursive: true });
    fs.mkdirSync(submoduleCwd, { recursive: true });
    fs.writeFileSync(
      path.join(worktreeRoot, '.zeroshot', 'tooling-env.json'),
      JSON.stringify({
        version: 1,
        worktreeRoot,
        toolBinDir,
      }),
      'utf8'
    );
    fs.writeFileSync(path.join(worktreeRoot, '.git'), 'gitdir: main-worktree\n', 'utf8');
    fs.writeFileSync(
      path.join(worktreeRoot, 'external', 'zeroshot', '.git'),
      'gitdir: nested-submodule\n',
      'utf8'
    );

    const runner = new ClaudeTaskRunner({ quiet: true });
    const originalPathEntries = (process.env.PATH || '').split(path.delimiter).filter(Boolean);

    const spawnEnv = runner._buildSpawnEnv('claude', null, {
      cwd: submoduleCwd,
      worktreePath: worktreeRoot,
    });

    const pathEntries = spawnEnv.PATH.split(path.delimiter);
    assert.strictEqual(pathEntries[0], toolBinDir);
    for (const entry of originalPathEntries) {
      assert.ok(pathEntries.includes(entry));
    }
  });

  it('forwards max reasoning effort into the detached task invocation', function () {
    const runner = new ClaudeTaskRunner({ quiet: true });
    const args = runner._buildRunArgs({
      context: 'test context',
      providerName: 'claude',
      runOutputFormat: 'stream-json',
      resolvedModelSpec: {
        model: 'claude-opus-4-8',
        reasoningEffort: 'max',
      },
      jsonSchema: null,
    });

    assert.deepStrictEqual(args.slice(args.indexOf('--model'), args.indexOf('--model') + 2), [
      '--model',
      'claude-opus-4-8',
    ]);
    assert.deepStrictEqual(
      args.slice(args.indexOf('--reasoning-effort'), args.indexOf('--reasoning-effort') + 2),
      ['--reasoning-effort', 'max']
    );
  });
});
