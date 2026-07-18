const assert = require('assert');
const { spawnSync } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const projectRoot = path.resolve(__dirname, '..', '..');
const lintStagedWrapper = path.join(projectRoot, 'scripts', 'run-lint-staged-no-stash.js');

function run(command, args, options = {}) {
  return spawnSync(command, args, {
    cwd: options.cwd,
    env: options.env || process.env,
    encoding: 'utf8',
  });
}

function runGit(repoDir, args) {
  const result = run('git', args, { cwd: repoDir });
  assert.strictEqual(
    result.status,
    0,
    `git ${args.join(' ')} failed\nSTDOUT:\n${result.stdout}\nSTDERR:\n${result.stderr}`
  );
  return result.stdout;
}

function setupPartiallyStagedRepo() {
  const repoDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-lint-staged-'));
  const filePath = path.join(repoDir, 'example.txt');
  const formatterPath = path.join(repoDir, 'format-staged.cjs');
  const configPath = path.join(repoDir, 'lint-staged.config.cjs');

  runGit(repoDir, ['init']);
  runGit(repoDir, ['config', 'user.email', 'test@example.com']);
  runGit(repoDir, ['config', 'user.name', 'Test User']);
  fs.writeFileSync(filePath, 'base\nstaged-target\nmiddle\ntail\n');
  runGit(repoDir, ['add', 'example.txt']);
  runGit(repoDir, ['commit', '-m', 'base']);

  fs.writeFileSync(filePath, 'base\nstaged\nmiddle\ntail\n');
  runGit(repoDir, ['add', 'example.txt']);
  fs.writeFileSync(filePath, 'base\nstaged\nmiddle\nunstaged-tail\n');

  fs.writeFileSync(
    formatterPath,
    [
      "const fs = require('fs');",
      'for (const filePath of process.argv.slice(2)) {',
      "  const source = fs.readFileSync(filePath, 'utf8');",
      "  fs.writeFileSync(filePath, source.replace('staged\\n', 'formatted-staged\\n'));",
      '}',
      "if (process.env.FAIL_FORMATTER === '1') process.exit(1);",
      '',
    ].join('\n')
  );
  fs.writeFileSync(
    configPath,
    `module.exports = { '*.txt': ${JSON.stringify(`${process.execPath} ${formatterPath}`)} };\n`
  );

  return { repoDir, filePath, configPath };
}

function readTraceEvents(tracePath) {
  if (!fs.existsSync(tracePath)) return [];
  return fs
    .readFileSync(tracePath, 'utf8')
    .split('\n')
    .filter(Boolean)
    .map((line) => JSON.parse(line));
}

function assertNoStashCommand(tracePath) {
  const stashStarts = readTraceEvents(tracePath).filter(
    (event) =>
      event.event === 'start' &&
      Array.isArray(event.argv) &&
      event.argv.some((argument) => argument === 'stash')
  );
  assert.deepStrictEqual(stashStarts, [], 'lint-staged must not invoke git stash');
}

function getRecoveryPatchPath(repoDir) {
  const gitPath = runGit(repoDir, ['rev-parse', '--git-path', 'lint-staged_unstaged.patch']).trim();
  return path.isAbsolute(gitPath) ? gitPath : path.resolve(repoDir, gitPath);
}

describe('pre-commit lint-staged isolation', function () {
  this.timeout(15000);

  it('disables lint-staged backup stashes without exposing partially staged hunks', function () {
    const hook = fs.readFileSync(path.join(projectRoot, '.husky', 'pre-commit'), 'utf8');
    assert.match(hook, /^node scripts\/run-lint-staged-no-stash\.js$/m);
    assert.doesNotMatch(hook, /--no-hide-partially-staged/);
  });

  it('preserves unstaged hunks while formatting only the staged snapshot', function () {
    const fixture = setupPartiallyStagedRepo();
    const tracePath = path.join(fixture.repoDir, 'git-trace.jsonl');
    try {
      const result = run(process.execPath, [lintStagedWrapper, '--config', fixture.configPath], {
        cwd: fixture.repoDir,
        env: { ...process.env, GIT_TRACE2_EVENT: tracePath },
      });
      assert.strictEqual(
        result.status,
        0,
        `lint-staged failed\nSTDOUT:\n${result.stdout}\nSTDERR:\n${result.stderr}`
      );
      assert.strictEqual(
        runGit(fixture.repoDir, ['show', ':example.txt']),
        'base\nformatted-staged\nmiddle\ntail\n'
      );
      assert.strictEqual(
        fs.readFileSync(fixture.filePath, 'utf8'),
        'base\nformatted-staged\nmiddle\nunstaged-tail\n'
      );
      assert.strictEqual(fs.existsSync(getRecoveryPatchPath(fixture.repoDir)), false);
      assertNoStashCommand(tracePath);
    } finally {
      fs.rmSync(fixture.repoDir, { recursive: true, force: true });
    }
  });

  it('fails closed without losing the unstaged hunk when a task rejects', function () {
    const fixture = setupPartiallyStagedRepo();
    const tracePath = path.join(fixture.repoDir, 'git-trace.jsonl');
    try {
      const result = run(process.execPath, [lintStagedWrapper, '--config', fixture.configPath], {
        cwd: fixture.repoDir,
        env: { ...process.env, FAIL_FORMATTER: '1', GIT_TRACE2_EVENT: tracePath },
      });
      assert.notStrictEqual(result.status, 0, 'a failing lint-staged task must abort the commit');
      assert.strictEqual(
        runGit(fixture.repoDir, ['show', ':example.txt']),
        'base\nformatted-staged\nmiddle\ntail\n'
      );
      assert.strictEqual(
        fs.readFileSync(fixture.filePath, 'utf8'),
        'base\nformatted-staged\nmiddle\nunstaged-tail\n'
      );
      assert.strictEqual(fs.existsSync(getRecoveryPatchPath(fixture.repoDir)), false);
      assertNoStashCommand(tracePath);
    } finally {
      fs.rmSync(fixture.repoDir, { recursive: true, force: true });
    }
  });

  it('refuses to overwrite unresolved recovery evidence', function () {
    const fixture = setupPartiallyStagedRepo();
    const recoveryPatchPath = getRecoveryPatchPath(fixture.repoDir);
    try {
      fs.writeFileSync(recoveryPatchPath, 'previous recovery evidence\n');
      const result = run(process.execPath, [lintStagedWrapper, '--config', fixture.configPath], {
        cwd: fixture.repoDir,
      });

      assert.notStrictEqual(result.status, 0);
      assert.match(result.stderr, /Refusing to overwrite unresolved unstaged changes/);
      assert.strictEqual(
        fs.readFileSync(recoveryPatchPath, 'utf8'),
        'previous recovery evidence\n'
      );
      assert.strictEqual(
        fs.readFileSync(fixture.filePath, 'utf8'),
        'base\nstaged\nmiddle\nunstaged-tail\n'
      );
    } finally {
      fs.rmSync(fixture.repoDir, { recursive: true, force: true });
    }
  });
});
