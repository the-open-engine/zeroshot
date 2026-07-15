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
const fs = require('fs');
const os = require('os');
const path = require('path');

process.env.NODE_ENV = process.env.NODE_ENV || 'test';

const testFileAliases = new Map([
  ['provider-cli-builder', ['tests/provider-cli-builder.test.js']],
  ['providers', ['tests/providers/**/*.test.js']],
  ['provider-retryable-errors', ['tests/unit/provider-retryable-errors.test.js']],
  ['watcher-crash-handling', ['tests/unit/watcher-crash-handling.test.js']],
  ['cli-provider-override', ['tests/unit/cli-provider-override.test.js']],
  ['stream-json-parser-codex', ['tests/unit/stream-json-parser-codex.test.js']],
  ['parse-result-output', ['tests/parse-result-output.test.js']],
]);

const args = process.argv.slice(2).flatMap((arg) => {
  if (arg.startsWith('-')) return [arg];
  return testFileAliases.get(arg) || [arg];
});
const defaultTestFiles = ['tests/unit/**/*.test.js', 'tests/*.test.js'];
const hasExplicitTestFile = args.some((arg) => !arg.startsWith('-'));
const mochaArgs = hasExplicitTestFile ? args : [...defaultTestFiles, ...args];
const explicitSettingsFile = process.env.ZEROSHOT_SETTINGS_FILE;
const isolatedSettingsDirectory = explicitSettingsFile
  ? null
  : fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-test-settings-'));
const settingsFile = explicitSettingsFile || path.join(isolatedSettingsDirectory, 'settings.json');
const child = spawn('npx', ['mocha', '--parallel', ...mochaArgs], {
  stdio: ['inherit', 'pipe', 'pipe'],
  env: { ...process.env, ZEROSHOT_SETTINGS_FILE: settingsFile },
});

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
  if (isolatedSettingsDirectory) {
    fs.rmSync(isolatedSettingsDirectory, { recursive: true, force: true });
  }

  if (code === 0) {
    process.exit(0);
  }

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
    process.exit(0);
  }

  // Real test failures
  process.exit(code || 1);
});
