#!/usr/bin/env node
/**
 * Postinstall check: warn if the npm global bin dir isn't on PATH.
 *
 * Runs after `npm install -g` regardless of whether the user can invoke
 * `zeroshot` afterward (this is the only guaranteed-execution point in
 * that failure scenario).
 */

const { checkBinDirOnPath, printPathWarning } = require('../lib/path-check');

try {
  const { onPath, binDir } = checkBinDirOnPath();
  if (!onPath && binDir) {
    printPathWarning(binDir);
  }
} catch (err) {
  console.warn(`[postinstall] Warning: PATH check failed: ${err.message}`);
}
