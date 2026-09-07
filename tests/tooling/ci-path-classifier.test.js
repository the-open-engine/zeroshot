'use strict';

const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const path = require('node:path');
const { describe, it } = require('node:test');

const { classifyPath, classifyPaths } = require('../../.github/ci-path-classifier');

const projectRoot = path.resolve(__dirname, '..', '..');
const classifierPath = path.join(projectRoot, '.github', 'ci-path-classifier.js');

describe('CI path classifier', () => {
  it('selects the native product and dependent Python SDK for native changes', () => {
    assert.equal(classifyPath('zeroshot/src/main.rs'), 'native');
    assert.equal(classifyPath('npm/zeroshot/install.js'), 'native');
    assert.deepEqual(classifyPaths(['zeroshot/src/main.rs']), {
      native: true,
      python: true,
      tooling: true,
      ownership: {
        native: ['zeroshot/src/main.rs'],
        python: [],
        tooling: [],
        shared: [],
      },
    });
  });

  it('keeps Python-only changes independent from native builds', () => {
    assert.equal(classifyPath('sdks/python/src/zeroshot/client.py'), 'python');
    assert.deepEqual(classifyPaths(['sdks/python/src/zeroshot/client.py']), {
      native: false,
      python: true,
      tooling: false,
      ownership: {
        native: [],
        python: ['sdks/python/src/zeroshot/client.py'],
        tooling: [],
        shared: [],
      },
    });
  });

  it('runs Python and tooling checks for the Python release workflow', () => {
    assert.deepEqual(classifyPaths(['.github/workflows/release-python.yml']), {
      native: false,
      python: true,
      tooling: true,
      ownership: {
        native: [],
        python: ['.github/workflows/release-python.yml'],
        tooling: ['.github/workflows/release-python.yml'],
        shared: [],
      },
    });
  });

  it('runs every lane for repository-wide or unknown changes', () => {
    for (const pathname of ['.github/workflows/ci.yml', 'new-surface/file']) {
      const result = classifyPaths([pathname]);
      assert.deepEqual(
        { native: result.native, python: result.python, tooling: result.tooling },
        { native: true, python: true, tooling: true }
      );
    }
    assert.deepEqual(
      (({ native, python, tooling }) => ({ native, python, tooling }))(classifyPaths([])),
      { native: true, python: true, tooling: true }
    );
  });

  it('emits GitHub outputs from null-delimited paths', () => {
    const result = spawnSync(process.execPath, [classifierPath], {
      cwd: projectRoot,
      encoding: 'utf8',
      input: 'zeroshot/src/main.rs\0npm/zeroshot/install.js\0',
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, 'native=true\npython=true\ntooling=true\n');
  });
});
