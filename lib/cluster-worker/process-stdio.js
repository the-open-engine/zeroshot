'use strict';

const util = require('node:util');

function redirectConsoleToStderr() {
  console.log = (...args) => process.stderr.write(`${util.format(...args)}\n`);
  console.info = console.log;
  console.debug = console.log;
}

function bindProcessLifecycle(runtime) {
  const closeAndExit = () => {
    runtime.close().finally(() => process.exit(0));
  };
  process.stdin.once('end', closeAndExit);
  for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, closeAndExit);
}

module.exports = { bindProcessLifecycle, redirectConsoleToStderr };
