const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');

const {
  prependWorktreeToolBinToEnv,
  resolveWorktreeToolBinEntries,
} = require('../../src/worktree-tooling-env');

describe('worktree-tooling-env', function () {
  /** @type {string[]} */
  let tempDirs = [];

  afterEach(function () {
    for (const dir of tempDirs.splice(0)) {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  it('prepends persisted worktree tool bins to PATH for nested cwd values', function () {
    const worktreeRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-worktree-tools-'));
    tempDirs.push(worktreeRoot);

    const toolBinDir = path.join(worktreeRoot, '.zeroshot', 'bin');
    fs.mkdirSync(toolBinDir, { recursive: true });
    fs.writeFileSync(
      path.join(worktreeRoot, '.zeroshot', 'tooling-env.json'),
      JSON.stringify({
        version: 1,
        worktreeRoot,
        toolBinDir,
      }),
      'utf8'
    );
    fs.writeFileSync(path.join(worktreeRoot, '.git'), 'gitdir: test\n', 'utf8');
    fs.mkdirSync(path.join(worktreeRoot, 'nested', 'dir'), { recursive: true });

    const env = { PATH: `/usr/bin${path.delimiter}/bin` };
    prependWorktreeToolBinToEnv(env, {
      cwd: path.join(worktreeRoot, 'nested', 'dir'),
    });

    const pathEntries = env.PATH.split(path.delimiter);
    assert.strictEqual(pathEntries[0], toolBinDir);
    assert.ok(pathEntries.includes('/usr/bin'));
    assert.deepStrictEqual(resolveWorktreeToolBinEntries({ cwd: worktreeRoot }), [toolBinDir]);
  });

  it('prefers the ancestor with tooling metadata over nested submodule .git roots', function () {
    const worktreeRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-worktree-tools-'));
    tempDirs.push(worktreeRoot);

    const toolBinDir = path.join(worktreeRoot, '.zeroshot', 'bin');
    const submoduleRoot = path.join(worktreeRoot, 'external', 'zeroshot');
    const submoduleNestedDir = path.join(submoduleRoot, 'src');
    fs.mkdirSync(toolBinDir, { recursive: true });
    fs.mkdirSync(submoduleNestedDir, { recursive: true });
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
    fs.writeFileSync(path.join(submoduleRoot, '.git'), 'gitdir: nested-submodule\n', 'utf8');

    const env = { PATH: `/usr/bin${path.delimiter}/bin` };
    prependWorktreeToolBinToEnv(env, { cwd: submoduleNestedDir });

    const pathEntries = env.PATH.split(path.delimiter);
    assert.strictEqual(pathEntries[0], toolBinDir);
    assert.deepStrictEqual(resolveWorktreeToolBinEntries({ cwd: submoduleNestedDir }), [toolBinDir]);
  });

  it('ignores symlinked tool bins that escape the worktree root', function () {
    const worktreeRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-worktree-tools-'));
    const outsideDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-shared-tools-'));
    tempDirs.push(worktreeRoot, outsideDir);

    const toolBinLink = path.join(worktreeRoot, '.zeroshot', 'bin');
    fs.mkdirSync(path.dirname(toolBinLink), { recursive: true });
    fs.symlinkSync(outsideDir, toolBinLink, 'dir');
    fs.writeFileSync(
      path.join(worktreeRoot, '.zeroshot', 'tooling-env.json'),
      JSON.stringify({
        version: 1,
        worktreeRoot,
        toolBinDir: toolBinLink,
      }),
      'utf8'
    );
    fs.writeFileSync(path.join(worktreeRoot, '.git'), 'gitdir: test\n', 'utf8');

    const env = { PATH: `/usr/bin${path.delimiter}/bin` };
    prependWorktreeToolBinToEnv(env, { cwd: worktreeRoot });

    assert.strictEqual(env.PATH, `/usr/bin${path.delimiter}/bin`);
    assert.deepStrictEqual(resolveWorktreeToolBinEntries({ cwd: worktreeRoot }), []);
  });
});
