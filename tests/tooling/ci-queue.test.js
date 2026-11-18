'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { it } = require('node:test');
const yaml = require('js-yaml');
const { classifyPaths } = require('../../.github/ci-path-classifier');

const root = path.resolve(__dirname, '..', '..');
const readWorkflow = (name) =>
  yaml.load(fs.readFileSync(path.join(root, '.github/workflows', name), 'utf8'));
const ci = readWorkflow('ci.yml');
const fullJobs = [
  'tooling-check',
  'npm-check',
  'native-check',
  'macos-check',
  'python-check',
  'docs-check',
  'target-image',
  'coverage-check',
  'opcore-check',
];

it('preserves the native job names and owned prefixes consumed by support queue admission', () => {
  assert.equal(ci.jobs['native-check'].name, 'Native product (${{ matrix.os }})');
  assert.deepEqual(ci.jobs['native-check'].strategy.matrix.os, ['ubuntu-latest', 'windows-latest']);
  assert.equal(ci.jobs['macos-check'].name, 'macOS compilation');
  assert.equal(ci.jobs['target-image'].name, 'Target image');
  for (const prefix of ['crates/', 'docker/zeroshot-target/', 'protocol/', 'ui/', 'zeroshot/']) {
    assert.equal(classifyPaths([prefix + 'regression']).native, true, prefix);
  }
});

it('runs full CI once in the merge queue and preserves manual full validation', () => {
  assert.equal(Object.hasOwn(ci.on, 'push'), false);
  assert.deepEqual(ci.on.merge_group.types, ['checks_requested']);
  assert.equal(Object.hasOwn(ci.on, 'workflow_dispatch'), true);
  assert.equal(ci.jobs['quick-check'].if, "github.event_name == 'pull_request'");
  assert.equal(ci.jobs.required.name, 'required');
  assert.equal(ci.jobs.required.if, 'always()');
  for (const job of fullJobs) {
    assert.match(ci.jobs[job].if, /^github\.event_name != 'pull_request'/, job);
    assert.ok(ci.jobs.required.needs.includes(job), job);
  }
  assert.ok(ci.jobs.required.needs.includes('quick-check'));
  for (const [job, filename] of [
    ['coverage-check', 'coverage.yml'],
    ['opcore-check', 'opcore.yml'],
  ]) {
    assert.equal(ci.jobs[job].uses, `./.github/workflows/${filename}`);
    assert.deepEqual(Object.keys(readWorkflow(filename).on).sort(), [
      'workflow_call',
      'workflow_dispatch',
    ]);
  }
});

function environment(event) {
  const gate = ci.jobs.required.steps[0];
  const env = Object.fromEntries(
    Object.keys(gate.env).map((key) => [key, key.endsWith('_SELECTED') ? 'true' : 'success'])
  );
  env.EVENT_NAME = event;
  env.QUICK_RESULT = event === 'pull_request' ? 'success' : 'skipped';
  if (event === 'pull_request') {
    for (const key of Object.keys(env)) {
      if (
        key.endsWith('_RESULT') &&
        !['CLASSIFY_RESULT', 'QUICK_RESULT', 'README_RESULT'].includes(key)
      ) {
        env[key] = 'skipped';
      }
    }
  }
  return env;
}

function accepted(env) {
  const result = spawnSync('bash', ['-c', ci.jobs.required.steps[0].run], {
    encoding: 'utf8',
    env: { ...process.env, ...env },
  });
  assert.ifError(result.error);
  return result.status === 0;
}

it(
  'requires the real PR checks and forbids full jobs on a pull request',
  { skip: process.platform === 'win32' },
  () => {
    const env = environment('pull_request');
    assert.equal(accepted(env), true);
    for (const key of ['CLASSIFY_RESULT', 'QUICK_RESULT', 'README_RESULT']) {
      for (const result of ['failure', 'cancelled', 'skipped', '']) {
        assert.equal(accepted({ ...env, [key]: result }), false, `${key}: ${result}`);
      }
    }
    for (const key of ['NATIVE_RESULT', 'COVERAGE_RESULT', 'OPCORE_RESULT']) {
      assert.equal(accepted({ ...env, [key]: 'success' }), false, key);
    }
    assert.equal(accepted({ ...env, NATIVE_SELECTED: '' }), false);
  }
);

it(
  'fails the queue gate for every missing, failed or unexpectedly skipped full check',
  { skip: process.platform === 'win32' },
  () => {
    for (const event of ['merge_group', 'workflow_dispatch']) {
      const env = environment(event);
      assert.equal(accepted(env), true, event);
      for (const key of Object.keys(env).filter(
        (name) => name.endsWith('_RESULT') && name !== 'QUICK_RESULT'
      )) {
        for (const result of ['failure', 'cancelled', 'skipped', '']) {
          assert.equal(accepted({ ...env, [key]: result }), false, `${event}: ${key}: ${result}`);
        }
      }
      assert.equal(accepted({ ...env, QUICK_RESULT: 'success' }), false);
      assert.equal(accepted({ ...env, NATIVE_SELECTED: 'unknown' }), false);
      assert.equal(accepted({ ...env, NATIVE_SELECTED: 'false' }), false);
      assert.equal(accepted({ ...env, EVENT_NAME: 'push' }), false);
    }
  }
);

it('compiles the same Windows UI test artifacts before starting the bounded execution host', () => {
  const steps = ci.jobs['native-check'].steps;
  const prebuild = steps.find((step) => step.run?.includes('test-windows.ps1 -Ui -BuildOnly'));
  const execution = steps.find((step) => step.run === './.github/scripts/test-windows-host.ps1');
  assert.ok(prebuild, 'initial UI test compilation must happen before the host deadline starts');
  assert.ok(execution, 'Windows tests must retain their independent native execution host');
  assert.equal(prebuild.if, "runner.os == 'Windows'");
  assert.equal(execution.if, prebuild.if);
  assert.ok(steps.indexOf(prebuild) < steps.indexOf(execution));

  const script = fs.readFileSync(path.join(root, 'scripts/test-windows.ps1'), 'utf8');
  assert.match(script, /param\(\[switch\]\$Ui, \[switch\]\$BuildOnly\)/);
  const build = script.indexOf('cargo test --workspace @features --jobs 1 --no-run');
  const buildFailure = script.indexOf('if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }');
  const emptyArtifacts = script.indexOf(
    "if ($tests.Count -eq 0) { throw 'Cargo produced no workspace test executables' }"
  );
  const buildOnly = script.indexOf('if ($BuildOnly) { exit 0 }');
  const runTests = script.indexOf('foreach ($test in $tests)');
  const doctests = script.indexOf('cargo test --workspace @features --jobs 1 --doc');
  assert.ok(build >= 0 && build < buildFailure && buildFailure < emptyArtifacts);
  assert.ok(emptyArtifacts < buildOnly && buildOnly < runTests && runTests < doctests);
  assert.match(script, /\$features = if \(\$Ui\) \{ @\('--features', 'zeroshot\/ui'\) \}/);

  const host = fs.readFileSync(path.join(root, '.github/scripts/test-windows-host.ps1'), 'utf8');
  assert.match(host, /-File \.\/scripts\/test-windows\.ps1 -Ui \*>/);
  assert.match(host, /Invoke-CimMethod -ClassName Win32_Process -MethodName Create/);
  assert.match(host, /WaitForExit\(1800000\)/);
  assert.match(host, /\$process\.Kill\(\$true\)/);
});

it('preserves Windows prebuild compiler settings without persisting registry credentials', () => {
  const host = fs.readFileSync(path.join(root, '.github/scripts/test-windows-host.ps1'), 'utf8');
  const names = new Set(
    [...host.match(/\$names = @\(([\s\S]*?)\n\)/)[1].matchAll(/'([^']+)'/g)].map(
      (match) => match[1]
    )
  );
  const buildNames = new RegExp(host.match(/\$_ -match '([^']+)'/)[1], 'i');
  const carried = (name) => names.has(name) || buildNames.test(name);
  for (const name of [
    'PATH',
    'HOME',
    'CARGO_HOME',
    'RUSTUP_HOME',
    'RUSTUP_TOOLCHAIN',
    'CARGO_INCREMENTAL',
    'CARGO_TARGET_DIR',
    'CARGO_BUILD_JOBS',
    'CARGO_BUILD_TARGET',
    'CARGO_TERM_COLOR',
    'RUSTFLAGS',
    'CARGO_ENCODED_RUSTFLAGS',
    'RUSTDOCFLAGS',
    'CARGO_ENCODED_RUSTDOCFLAGS',
    'RUSTC',
    'RUSTDOC',
    'RUSTC_WRAPPER',
    'RUSTC_WORKSPACE_WRAPPER',
    'CARGO_BUILD_RUSTFLAGS',
    'CARGO_BUILD_RUSTDOCFLAGS',
    'CARGO_BUILD_RUSTC',
    'CARGO_BUILD_RUSTDOC',
    'CARGO_BUILD_RUSTC_WRAPPER',
    'CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER',
    'CARGO_PROFILE_TEST_DEBUG',
    'CARGO_PROFILE_DEV_BUILD_OVERRIDE_OPT_LEVEL',
    'CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER',
    'CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS',
  ]) {
    assert.equal(carried(name), true, name);
  }
  for (const name of [
    'CARGO_REGISTRY_TOKEN',
    'CARGO_REGISTRY_CREDENTIAL_PROVIDER',
    'CARGO_REGISTRIES_PRIVATE_TOKEN',
    'CARGO_REGISTRIES_PRIVATE_INDEX',
    'CARGO_REGISTRIES_PRIVATE_CREDENTIAL_PROVIDER',
    'GITHUB_TOKEN',
    'CARGO_PROFILE_TEST_REGISTRY_TOKEN',
    'CARGO_TARGET_SECRET_TOKEN',
  ]) {
    assert.equal(carried(name), false, name);
  }
  assert.match(
    host,
    /\$names \+= @\(\s*\[Environment\]::GetEnvironmentVariables\('Process'\)\.Keys/
  );
  assert.match(host, /foreach \(\$name in \$names\)/);
  assert.match(host, /\[Environment\]::GetEnvironmentVariable\(\$name\)/);
  assert.match(host, /Environment = \$environment/);
  assert.match(host, /\$context\.Environment\.GetEnumerator\(\)/);
});
