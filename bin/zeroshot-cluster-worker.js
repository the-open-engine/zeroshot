#!/usr/bin/env node
'use strict';

const {
  bindProcessLifecycle,
  redirectConsoleToStderr,
} = require('../lib/cluster-worker/process-stdio');

// stdout is the worker protocol transport. Route ambient engine logging away
// from it before constructing the production adapter.
redirectConsoleToStderr();

const { runClusterWorkerExecutable } = require('../lib/cluster-worker/executable');

const runtime = runClusterWorkerExecutable();
bindProcessLifecycle(runtime);
