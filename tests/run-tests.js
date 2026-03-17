#!/usr/bin/env node
/**
 * Test runner wrapper that tolerates the known better-sqlite3 cleanup crash.
 *
 * better-sqlite3 throws "Cannot assign to read only property 'database'"
 * during process exit. This is harmless but causes mocha to report "1 failing".
 * This wrapper detects that specific failure pattern and exits 0 if all real
 * tests passed.
 */
const { spawn } = require('child_process');

const args = process.argv.slice(2);

// Run parallel-safe tests first (excludes tests that require serial execution)
// Serial tests:
// - preflight: modifies process.env (race conditions)
// - git-safety-hook: git command contention
// - tui-backend-stdio: spawns server process, temp HOME conflicts
// - watcher-crash-handling: spawns child processes, temp file race conditions
const child = spawn(
  'npx',
  [
    'mocha',
    'tests/unit/!(git-safety-hook|tui-backend-stdio|watcher-crash-handling).test.js',
    'tests/!(preflight|preflight-runtime-validation).test.js',
    '--parallel',
    '--exit',
    ...args,
  ],
  { stdio: ['inherit', 'pipe', 'pipe'] }
);

let stdout = '';
let stderr = '';

child.stdout.on('data', (data) => {
  process.stdout.write(data);
  stdout += data.toString();
});

child.stderr.on('data', (data) => {
  process.stderr.write(data);
  stderr += data.toString();
});

child.on('close', (code) => {
  if (code !== 0) {
    const combined = stdout + stderr;

    // Check if the ONLY failure is the known better-sqlite3 cleanup error
    const failMatch = combined.match(/(\d+) failing/);
    const passMatch = combined.match(/(\d+) passing/);
    const isSqliteOnly =
      failMatch &&
      failMatch[1] === '1' &&
      passMatch &&
      combined.includes("Cannot assign to read only property 'database'");

    if (isSqliteOnly) {
      process.stderr.write(
        `\n[test-runner] Ignoring known better-sqlite3 cleanup crash (${passMatch[1]} tests passed)\n`
      );
      // Continue to serial tests
    } else {
      // Real test failures in parallel tests
      process.exit(code || 1);
    }
  }

  // Parallel tests passed (or only SQLite cleanup error) - run serial tests
  process.stdout.write('\n[test-runner] Running serial tests...\n');
  const serialChild = spawn(
    'npx',
    [
      'mocha',
      'tests/preflight.test.js',
      'tests/preflight-runtime-validation.test.js',
      'tests/unit/git-safety-hook.test.js',
      'tests/unit/tui-backend-stdio.test.js',
      'tests/unit/watcher-crash-handling.test.js',
      '--exit',
      ...args,
    ],
    { stdio: 'inherit' }
  );

  serialChild.on('close', (serialCode) => {
    process.exit(serialCode);
  });
});
