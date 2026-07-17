'use strict';

const util = require('node:util');

function redirectConsoleToStderr() {
  console.log = (...args) => process.stderr.write(`${util.format(...args)}\n`);
  console.info = console.log;
  console.debug = console.log;
}

function bindProcessLifecycle(runtime) {
  let closing = null;
  const closeAndRelease = () => {
    closing ||= Promise.resolve()
      .then(() => runtime.close())
      .then(
        () => {
          process.exitCode = 0;
          process.exit(0);
        },
        (error) => {
          process.stderr.write(`cluster-worker shutdown failed: ${error.message}\n`);
          process.exitCode = 1;
          process.exit(1);
        }
      );
  };
  const closeFromSignal = () => {
    process.stdin.pause();
    process.stdin.destroy();
    closeAndRelease();
  };
  process.stdin.once('end', closeAndRelease);
  for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, closeFromSignal);
}

module.exports = { bindProcessLifecycle, redirectConsoleToStderr };
