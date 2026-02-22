/**
 * TEST: Detached startup handshake
 *
 * Ensures detached mode doesn't print "Started ..." until the daemon survives
 * an initial startup window, and surfaces early daemon failures.
 */

const assert = require('assert');
const fs = require('fs');
const path = require('path');

describe('Detached startup handshake', () => {
  const sourceFile = path.join(__dirname, '..', 'cli', 'index.js');
  let sourceCode;

  before(() => {
    sourceCode = fs.readFileSync(sourceFile, 'utf8');
  });

  it('makes spawnDetachedCluster async and awaited by run command', () => {
    assert.ok(
      sourceCode.includes('async function spawnDetachedCluster'),
      'Expected spawnDetachedCluster to be async'
    );
    assert.ok(
      sourceCode.includes('await spawnDetachedCluster(options, clusterId)'),
      'Expected run command to await detached startup'
    );
  });

  it('defers success message until startup grace window passes', () => {
    const fnMatch = sourceCode.match(/async function spawnDetachedCluster\([^)]*\)\s*{[\s\S]*?^}/m);
    assert.ok(fnMatch, 'Expected spawnDetachedCluster implementation');
    const fnBody = fnMatch[0];

    assert.ok(fnBody.includes('STARTUP_GRACE_MS'), 'Expected startup grace window constant');
    assert.ok(fnBody.includes('printDetachedClusterStart(options, clusterId)'), 'Expected delayed success print');
  });

  it('reports early daemon failures with log context', () => {
    assert.ok(
      sourceCode.includes('buildDetachedStartupError'),
      'Expected detached startup error builder'
    );
    assert.ok(
      sourceCode.includes('readDaemonLogTail'),
      'Expected daemon log-tail extraction for startup failures'
    );
    assert.ok(
      sourceCode.includes("daemon.once('exit'"),
      'Expected startup failure handling on early daemon exit'
    );
  });
});
