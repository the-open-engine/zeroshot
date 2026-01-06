#!/usr/bin/env node
/**
 * Fix node-pty spawn-helper permissions.
 *
 * node-pty prebuilds ship with spawn-helper lacking execute permission (mode 644).
 * This causes "posix_spawnp failed" errors on macOS/Linux.
 *
 * Upstream bug: https://github.com/microsoft/node-pty/issues/XXX
 * (File issue if not already reported)
 *
 * This script runs as postinstall to fix it automatically.
 */

const fs = require('fs');
const path = require('path');

// Skip on Windows - chmod doesn't apply
if (process.platform === 'win32') {
  process.exit(0);
}

const prebuildsDir = path.join(__dirname, '..', 'node_modules', 'node-pty', 'prebuilds');

if (!fs.existsSync(prebuildsDir)) {
  // node-pty not installed yet or using compiled version
  process.exit(0);
}

let fixed = 0;
let errors = 0;

try {
  const platforms = fs.readdirSync(prebuildsDir).filter(f => {
    try {
      return fs.statSync(path.join(prebuildsDir, f)).isDirectory();
    } catch {
      return false;
    }
  });

  for (const platform of platforms) {
    // Only fix Unix platforms (darwin, linux)
    if (!platform.startsWith('darwin') && !platform.startsWith('linux')) {
      continue;
    }

    const helper = path.join(prebuildsDir, platform, 'spawn-helper');
    try {
      if (!fs.existsSync(helper)) continue;

      const stat = fs.statSync(helper);
      // Check if not executable (missing user execute bit)
      if (!(stat.mode & 0o100)) {
        fs.chmodSync(helper, 0o755);
        fixed++;
      }
    } catch (err) {
      // Log but don't fail install - permission fix is best-effort
      console.warn(`[postinstall] Warning: Could not fix ${helper}: ${err.message}`);
      errors++;
    }
  }
} catch (err) {
  // Don't fail install on unexpected errors
  console.warn(`[postinstall] Warning: node-pty permission fix failed: ${err.message}`);
  process.exit(0);
}

if (fixed > 0) {
  console.log(`[postinstall] Fixed node-pty spawn-helper permissions (${fixed} platform(s))`);
}

if (errors > 0) {
  console.warn(`[postinstall] ${errors} platform(s) could not be fixed - may need manual chmod`);
}
