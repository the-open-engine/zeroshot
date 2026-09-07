'use strict';

const fs = require('node:fs');

const SHARED_PATHS = new Set(['.github/ci-path-classifier.js', '.github/workflows/ci.yml']);

const NATIVE_PREFIXES = [
  'crates/',
  'distribution/',
  'docker/zeroshot-target/',
  'docs/zeroshot-cli',
  'docs/zeroshot-distribution',
  'npm/zeroshot/',
  'protocol/',
  'zeroshot/',
];
const NATIVE_PATHS = new Set([
  '.github/workflows/release.yml',
  'Cargo.lock',
  'Cargo.toml',
  'clippy.toml',
  'rust-toolchain.toml',
  'rustfmt.toml',
  'scripts/distribution.js',
]);

const PYTHON_PREFIXES = ['sdks/python/'];
const PYTHON_PATHS = new Set(['.github/workflows/release-python.yml']);
const MULTI_LANE_PATHS = new Map([['.github/workflows/release-python.yml', ['python', 'tooling']]]);

const TOOLING_PREFIXES = [
  '.agents/',
  '.codex/',
  '.github/',
  '.husky/',
  '.opcore/',
  'scripts/',
  'tests/',
];
const TOOLING_PATHS = new Set([
  '.dockerignore',
  '.gitignore',
  '.prettierignore',
  '.prettierrc.json',
  'AGENTS.md',
  'CHANGELOG.md',
  'CONTRIBUTING.md',
  'PUBLISHING.md',
  'README.md',
  'commitlint.config.js',
  'eslint.config.mjs',
  'opcore-zero.docs.json',
  'package-lock.json',
  'package.json',
]);

function hasPrefix(pathname, prefixes) {
  return prefixes.some((prefix) => pathname.startsWith(prefix));
}

function normalizePath(pathname) {
  const value = String(pathname);
  return value.startsWith('./') ? value.slice(2) : value;
}

function classifyPath(pathname) {
  const normalized = normalizePath(pathname);
  if (SHARED_PATHS.has(normalized)) return 'shared';
  if (PYTHON_PATHS.has(normalized) || hasPrefix(normalized, PYTHON_PREFIXES)) return 'python';
  if (NATIVE_PATHS.has(normalized) || hasPrefix(normalized, NATIVE_PREFIXES)) return 'native';
  if (TOOLING_PATHS.has(normalized) || hasPrefix(normalized, TOOLING_PREFIXES)) return 'tooling';
  return 'shared';
}

function recordOwnership(ownership, selected, pathname, kinds) {
  for (const kind of kinds) {
    ownership[kind].push(pathname);
    selected.add(kind);
  }
}

function classifyPaths(paths) {
  const ownership = { native: [], python: [], tooling: [], shared: [] };
  const selected = new Set();

  for (const pathname of paths) {
    const normalized = normalizePath(pathname);
    if (!normalized) continue;
    const kinds = MULTI_LANE_PATHS.get(normalized) ?? [classifyPath(normalized)];
    recordOwnership(ownership, selected, normalized, kinds);
  }

  if (selected.size === 0) {
    return { native: true, python: true, tooling: true, ownership };
  }
  const shared = selected.has('shared');
  const native = selected.has('native') || shared;
  return {
    native,
    python: selected.has('python') || native || shared,
    tooling: selected.has('tooling') || native || shared,
    ownership,
  };
}

function changedPathsFromStdin() {
  const input = fs.readFileSync(0);
  if (input.length === 0) return [];
  const separator = input.includes(0) ? '\0' : /\r?\n/;
  return input.toString('utf8').split(separator).filter(Boolean);
}

function main() {
  const result = classifyPaths(changedPathsFromStdin());
  const { native, python, tooling, shared } = result.ownership;
  process.stdout.write(
    `native=${result.native}\npython=${result.python}\ntooling=${result.tooling}\n`
  );
  process.stderr.write(
    `CI ownership: native=${native.length}, python=${python.length}, ` +
      `tooling=${tooling.length}, shared=${shared.length}\n`
  );
}

module.exports = { classifyPath, classifyPaths };

if (require.main === module) main();
