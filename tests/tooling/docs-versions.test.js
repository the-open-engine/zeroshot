'use strict';

const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const path = require('node:path');
const { it } = require('node:test');

it('migrates published documentation and guards minor updates', () => {
  const result = spawnSync('python3', ['-m', 'unittest', 'discover', '-s', 'tests/docs', '-v'], {
    cwd: path.resolve(__dirname, '../..'),
    encoding: 'utf8',
  });
  assert.equal(result.status, 0, result.error?.message ?? result.stdout + result.stderr);
});
