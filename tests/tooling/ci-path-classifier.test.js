'use strict';

const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const path = require('node:path');
const { it } = require('node:test');

const { classifyPath, classifyPaths } = require('../../.github/ci-path-classifier');

const projectRoot = path.resolve(__dirname, '..', '..');
const classifierPath = path.join(projectRoot, '.github', 'ci-path-classifier.js');
const selections = (result) => {
  const { native, python, tooling, npm, docs } = result;
  return { native, python, tooling, npm, docs };
};

it('selects native checks and their established dependent lanes', () => {
  for (const pathname of [
    'zeroshot/src/main.rs',
    'ui/src/App.tsx',
    'ui/package-lock.json',
    '.dockerignore',
    'tests/tooling/smoke-ui.js',
  ]) {
    assert.equal(classifyPath(pathname), 'native');
  }
  assert.deepEqual(selections(classifyPaths(['zeroshot/src/main.rs'])), {
    native: true,
    python: true,
    tooling: true,
    npm: false,
    docs: false,
  });
});

it('keeps npm-only changes out of native, Python, and docs checks', () => {
  assert.equal(classifyPath('npm/zeroshot/install.js'), 'npm');
  assert.deepEqual(selections(classifyPaths(['npm/zeroshot/install.js'])), {
    native: false,
    python: false,
    tooling: false,
    npm: true,
    docs: false,
  });
});

it('keeps ordinary documentation changes in the strict docs lane', () => {
  assert.equal(classifyPath('docs/concepts/targets.md'), 'docs');
  assert.deepEqual(selections(classifyPaths(['docs/concepts/targets.md'])), {
    native: false,
    python: false,
    tooling: false,
    npm: false,
    docs: true,
  });
});

it('runs strict docs alongside direct Python SDK changes', () => {
  assert.equal(classifyPath('sdks/python/src/zeroshot/client.py'), 'python');
  assert.deepEqual(selections(classifyPaths(['sdks/python/src/zeroshot/client.py'])), {
    native: false,
    python: true,
    tooling: false,
    npm: false,
    docs: true,
  });
});

it('preserves explicit cross-lane ownership', () => {
  const cases = new Map([
    [
      '.github/workflows/release-python.yml',
      { native: false, python: true, tooling: true, npm: false, docs: true },
    ],
    [
      'distribution/zeroshot-targets.json',
      { native: true, python: true, tooling: true, npm: true, docs: false },
    ],
    [
      'scripts/distribution/artifacts.js',
      { native: true, python: true, tooling: true, npm: true, docs: false },
    ],
    ['docs/zeroshot-cli.md', { native: true, python: true, tooling: true, npm: false, docs: true }],
    [
      'docs/getting-started/install.md',
      { native: false, python: false, tooling: false, npm: false, docs: true },
    ],
    ['package-lock.json', { native: false, python: false, tooling: true, npm: true, docs: false }],
  ]);
  for (const [pathname, expected] of cases) {
    assert.deepEqual(selections(classifyPaths([pathname])), expected, pathname);
  }
});

it('runs every lane for release, CI, unknown, or unresolvable changes', () => {
  for (const paths of [
    ['.github/workflows/ci.yml'],
    ['.github/workflows/release.yml'],
    ['new-surface/file'],
    [],
  ]) {
    assert.deepEqual(selections(classifyPaths(paths)), {
      native: true,
      python: true,
      tooling: true,
      npm: true,
      docs: true,
    });
  }
});

it('emits every GitHub output from null-delimited paths', () => {
  const result = spawnSync(process.execPath, [classifierPath], {
    cwd: projectRoot,
    encoding: 'utf8',
    input: 'npm/zeroshot/install.js\0docs/concepts/targets.md\0',
  });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, 'native=false\npython=false\ntooling=false\nnpm=true\ndocs=true\n');
});
