#!/usr/bin/env node
'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawnSync } = require('child_process');
const { selectTarget } = require('../lib/install');

try {
  const { executable } = selectTarget();
  const binary = path.join(__dirname, 'native', executable);
  if (!fs.existsSync(binary)) {
    throw new Error(
      `NATIVE_BINARY_MISSING: ${binary}; reinstall @the-open-engine-company/zeroshot with lifecycle scripts enabled`
    );
  }
  const result = spawnSync(binary, process.argv.slice(2), { stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.signal) {
    // Node ignores some signals, such as SIGPIPE and SIGXFSZ, so the re-raise can return.
    // Then exit with the status a shell reports for that signal death.
    const number = os.constants.signals[result.signal];
    process.exitCode = number === undefined ? 1 : 128 + number;
    process.kill(process.pid, result.signal);
  } else {
    process.exitCode = result.status === null ? 1 : result.status;
  }
} catch (error) {
  process.stderr.write(`zeroshot: ${error.message}\n`);
  process.exitCode = 1;
}
