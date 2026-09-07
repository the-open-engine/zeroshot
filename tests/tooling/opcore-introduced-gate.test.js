const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFileSync, spawnSync } = require('child_process');
const { describe, it } = require('node:test');

const introducedGate = path.resolve(__dirname, '../../scripts/opcore-introduced-check.js');
const agentGate = path.resolve(__dirname, '../../scripts/opcore-agent-gate.js');

function oversizedFunction(name) {
  const statements = Array.from({ length: 81 }, (_, index) => `    let _value_${index} = 0;`);
  return [`pub fn ${name}() -> i32 {`, ...statements, '    0', '}', ''].join('\n');
}

function runOpcore(repo, args = [], checks = 'rust.function-metrics') {
  return spawnSync(process.execPath, [introducedGate, ...args, '--checks', checks], {
    cwd: repo,
    encoding: 'utf8',
    env: {
      ...process.env,
      GIT_INDEX_FILE: path.join(repo, '.git', 'index'),
    },
  });
}

function parseResult(run) {
  assert.ok(run.stdout, run.stderr);
  return JSON.parse(run.stdout);
}

function runAgentGate(content) {
  const repo = path.resolve(__dirname, '../..');
  const payload = JSON.stringify({
    tool_name: 'Write',
    tool_input: {
      file_path: 'tests/.tmp-opcore-agent-gate.rs',
      content,
    },
    cwd: repo,
  });
  return spawnSync(process.execPath, [agentGate, '--harness', 'codex', '--repo', repo], {
    cwd: repo,
    encoding: 'utf8',
    input: payload,
  });
}

function initializeRepo(repo, content) {
  fs.writeFileSync(path.join(repo, 'lib.rs'), content);
  execFileSync('git', ['init', '-q'], { cwd: repo });
  execFileSync('git', ['add', 'lib.rs'], { cwd: repo });
  execFileSync(
    'git',
    [
      '-c',
      'user.name=Zeroshot Test',
      '-c',
      'user.email=test@zeroshot.invalid',
      'commit',
      '-qm',
      'baseline',
    ],
    { cwd: repo }
  );
}

function baselineDebtCase() {
  const repo = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-opcore-introduced-'));
  const source = path.join(repo, 'lib.rs');
  try {
    initializeRepo(repo, oversizedFunction('existing_debt'));
    fs.appendFileSync(source, '// unrelated clean change\n');
    const unchangedDebt = runOpcore(repo);
    const unchangedDebtResult = parseResult(unchangedDebt);
    assert.strictEqual(unchangedDebt.status, 0, unchangedDebt.stderr);
    assert.strictEqual(unchangedDebtResult.validationResult.status, 'passed');
    assert.ok(
      unchangedDebtResult.validationResult.diagnostics.every(
        (diagnostic) => diagnostic.code !== 'RUST_FUNCTION_LINES'
      )
    );

    fs.appendFileSync(source, `\n${oversizedFunction('introduced_debt')}`);
    const introducedDebt = runOpcore(repo);
    const introducedDebtResult = parseResult(introducedDebt);
    assert.strictEqual(introducedDebt.status, 1, introducedDebt.stderr);
    assert.strictEqual(introducedDebtResult.validationResult.status, 'policy_failure');
    assert.ok(
      introducedDebtResult.validationResult.diagnostics.some(
        (diagnostic) =>
          diagnostic.code === 'RUST_FUNCTION_LINES' &&
          diagnostic.message.includes('introduced_debt')
      )
    );
  } finally {
    fs.rmSync(repo, { recursive: true, force: true });
  }
}

function stagedIndexCase() {
  const repo = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-opcore-staged-'));
  const source = path.join(repo, 'lib.rs');
  try {
    initializeRepo(repo, 'pub fn baseline() -> i32 {\n    0\n}\n');
    fs.writeFileSync(source, oversizedFunction('staged_violation'));
    execFileSync('git', ['add', 'lib.rs'], { cwd: repo });
    fs.writeFileSync(source, 'pub fn unstaged_replacement() -> i32 {\n    0\n}\n');

    const staged = runOpcore(repo, ['--staged']);
    const result = parseResult(staged);
    assert.strictEqual(staged.status, 1, staged.stderr);
    assert.ok(
      result.validationResult.diagnostics.some(
        (diagnostic) =>
          diagnostic.code === 'RUST_FUNCTION_LINES' &&
          diagnostic.message.includes('staged_violation')
      )
    );
  } finally {
    fs.rmSync(repo, { recursive: true, force: true });
  }
}

function deletedFileCase() {
  const repo = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-opcore-delete-'));
  const retained = path.join(repo, 'retained.rs');
  try {
    initializeRepo(repo, 'pub fn removed() -> i32 {\n    0\n}\n');
    fs.writeFileSync(retained, 'pub fn retained() -> i32 {\n    0\n}\n');
    execFileSync('git', ['add', 'retained.rs'], { cwd: repo });
    execFileSync(
      'git',
      [
        '-c',
        'user.name=Zeroshot Test',
        '-c',
        'user.email=test@zeroshot.invalid',
        'commit',
        '-qm',
        'add retained source',
      ],
      { cwd: repo }
    );
    fs.unlinkSync(path.join(repo, 'lib.rs'));
    fs.appendFileSync(retained, '// retained source remains in validation scope\n');

    const run = runOpcore(repo, [], 'rust.fmt');
    const result = parseResult(run);
    assert.strictEqual(run.status, 0, `${run.stderr}\n${run.stdout}`);
    assert.strictEqual(result.validationResult.status, 'passed');
    assert.ok(
      result.validationResult.diagnostics.every(
        (diagnostic) => !diagnostic.message.includes('lib.rs` does not exist')
      )
    );
  } finally {
    fs.rmSync(repo, { recursive: true, force: true });
  }
}
function deletedCloneSourceCase() {
  const repo = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-opcore-move-'));
  const source = [
    'pub fn moved() -> i32 {',
    '    let one = 1;',
    '    let two = 2;',
    '    let three = 3;',
    '    let four = 4;',
    '    one + two + three + four',
    '}',
    '',
  ].join('\n');
  try {
    initializeRepo(repo, source);
    fs.unlinkSync(path.join(repo, 'lib.rs'));
    fs.writeFileSync(path.join(repo, 'moved.rs'), source);

    const run = runOpcore(repo, [], 'clone.duplication');
    const result = parseResult(run);
    assert.strictEqual(run.status, 0, `${run.stderr}\n${run.stdout}`);
    assert.strictEqual(result.validationResult.status, 'passed');
    assert.ok(
      result.validationResult.diagnostics.every(
        (diagnostic) => diagnostic.code !== 'CLONE_DUPLICATE'
      )
    );
  } finally {
    fs.rmSync(repo, { recursive: true, force: true });
  }
}

function symbolicBaseCase() {
  const repo = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-opcore-symbolic-base-'));
  try {
    fs.writeFileSync(
      path.join(repo, 'Cargo.toml'),
      '[package]\nname = "symbolic-base"\nversion = "0.1.0"\nedition = "2021"\n\n[lib]\npath = "lib.rs"\n'
    );
    fs.writeFileSync(path.join(repo, 'lib.rs'), 'pub fn value() -> i32 {\n    0\n}\n');
    execFileSync('git', ['init', '-q'], { cwd: repo });
    execFileSync('git', ['add', '.'], { cwd: repo });
    execFileSync(
      'git',
      [
        '-c',
        'user.name=Zeroshot Test',
        '-c',
        'user.email=test@zeroshot.invalid',
        'commit',
        '-qm',
        'old main',
      ],
      { cwd: repo }
    );
    const oldMain = execFileSync('git', ['rev-parse', 'HEAD'], {
      cwd: repo,
      encoding: 'utf8',
    }).trim();
    execFileSync('git', ['branch', 'main', oldMain], { cwd: repo });
    fs.writeFileSync(path.join(repo, 'support.rs'), 'pub fn value() -> i32 {\n    1\n}\n');
    execFileSync('git', ['add', 'support.rs'], { cwd: repo });
    execFileSync(
      'git',
      [
        '-c',
        'user.name=Zeroshot Test',
        '-c',
        'user.email=test@zeroshot.invalid',
        'commit',
        '-qm',
        'remote main',
      ],
      { cwd: repo }
    );
    const remoteMain = execFileSync('git', ['rev-parse', 'HEAD'], {
      cwd: repo,
      encoding: 'utf8',
    }).trim();
    execFileSync('git', ['update-ref', 'refs/remotes/origin/main', remoteMain], { cwd: repo });
    fs.writeFileSync(
      path.join(repo, 'lib.rs'),
      'mod support;\n\npub fn value() -> i32 {\n    support::value()\n}\n'
    );

    const run = runOpcore(repo, ['--base', 'origin/main'], 'rust.cargo-check');
    const result = parseResult(run);
    assert.strictEqual(run.status, 0, `${run.stderr}\n${run.stdout}`);
    assert.strictEqual(result.validationResult.status, 'passed');
  } finally {
    fs.rmSync(repo, { recursive: true, force: true });
  }
}

function agentGateCase() {
  const clean = runAgentGate('pub fn clean() -> i32 {\n    0\n}\n');
  assert.strictEqual(clean.status, 0, clean.stderr);

  const violation = runAgentGate(oversizedFunction('introduced_by_agent'));
  assert.strictEqual(violation.status, 2, violation.stderr);
  assert.match(violation.stderr, /status=policy_failure/);
  assert.doesNotMatch(violation.stderr, /timed out/i);
}

describe('Opcore introduced-change gate', { timeout: 90000 }, function () {
  it('ignores baseline debt but blocks a newly introduced violation', baselineDebtCase);
  it('validates the staged index rather than an unstaged replacement', stagedIndexCase);
  it('does not validate a path after it is deleted', deletedFileCase);
  it('removes deleted sources before checking moved code for clones', deletedCloneSourceCase);
  it('pins symbolic base refs before cloning the validation tree', symbolicBaseCase);
  it(
    'allows a clean pre-write and blocks an introduced violation within its deadline',
    agentGateCase
  );
});
