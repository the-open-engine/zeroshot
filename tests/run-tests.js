#!/usr/bin/env node
/** Test runner wrapper for Mocha's unit-test file selection. */
const { spawn } = require('child_process');

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
const child = spawn('npx', ['mocha', '--parallel', ...mochaArgs], {
  stdio: 'inherit',
});

child.on('close', (code) => {
  process.exit(code ?? 1);
});
