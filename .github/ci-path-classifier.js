'use strict';

const fs = require('node:fs');

const SHARED_PATHS = new Set([
  '.github/ci-path-classifier.js',
  '.github/workflows/ci.yml',
  '.github/workflows/release.yml',
]);

const NATIVE_PREFIXES = ['crates/', 'docker/zeroshot-target/', 'protocol/', 'ui/', 'zeroshot/'];
const NATIVE_PATHS = new Set([
  '.dockerignore',
  '.github/scripts/test-windows-host.ps1',
  'Cargo.lock',
  'Cargo.toml',
  'clippy.toml',
  'rust-toolchain.toml',
  'rustfmt.toml',
  'scripts/test-windows.ps1',
  'tests/tooling/smoke-ui.js',
]);

const PYTHON_PREFIXES = ['sdks/python/'];
const PYTHON_PATHS = new Set();

const NPM_PREFIXES = ['npm/zeroshot/'];
const NPM_PATHS = new Set();

const DOCS_PREFIXES = ['docs/'];
const DOCS_PATHS = new Set([
  '.github/workflows/docs.yml',
  'mkdocs.yml',
  'scripts/docs_hook.py',
  'scripts/docs_versions.py',
  'tests/tooling/docs-versions.test.js',
]);

const TOOLING_PREFIXES = ['.github/', '.husky/', 'scripts/', 'tests/'];
const TOOLING_PATHS = new Set([
  '.gitignore',
  '.prettierignore',
  '.prettierrc.json',
  'AGENTS.md',
  'CHANGELOG.md',
  'CONTRIBUTING.md',
  'PUBLISHING.md',
  'README.md',
  'commitlint.config.js',
]);

const MULTI_LANE_PATHS = new Map([
  ['.github/workflows/release-python.yml', ['python', 'tooling']],
  ['distribution/zeroshot-targets.json', ['native', 'npm']],
  ['docs/reference/cluster/api.md', ['native', 'docs']],
  ['docs/zeroshot-cli.html', ['native', 'docs']],
  ['docs/zeroshot-cli.md', ['native', 'docs']],
  ['eslint.config.mjs', ['tooling', 'npm']],
  ['package-lock.json', ['tooling', 'npm']],
  ['package.json', ['tooling', 'npm']],
  ['scripts/distribution.js', ['native', 'npm']],
  ['tests/tooling/distribution.test.js', ['npm']],
  ['tests/tooling/npm-package-install.test.js', ['npm']],
  ['tests/tooling/npm-shim.test.js', ['npm']],
  ['tests/tooling/npm-skill-install.test.js', ['npm']],
]);
const MULTI_LANE_PREFIXES = [['scripts/distribution/', ['native', 'npm']]];

function hasPrefix(pathname, prefixes) {
  return prefixes.some((prefix) => pathname.startsWith(prefix));
}

function normalizePath(pathname) {
  const value = String(pathname);
  return value.startsWith('./') ? value.slice(2) : value;
}

const SINGLE_LANES = [
  ['python', PYTHON_PATHS, PYTHON_PREFIXES],
  ['native', NATIVE_PATHS, NATIVE_PREFIXES],
  ['npm', NPM_PATHS, NPM_PREFIXES],
  ['docs', DOCS_PATHS, DOCS_PREFIXES],
  ['tooling', TOOLING_PATHS, TOOLING_PREFIXES],
];

function classifyPath(pathname) {
  const normalized = normalizePath(pathname);
  if (SHARED_PATHS.has(normalized)) return 'shared';
  for (const [lane, paths, prefixes] of SINGLE_LANES) {
    if (paths.has(normalized) || hasPrefix(normalized, prefixes)) return lane;
  }
  return 'shared';
}

function laneKinds(pathname) {
  const exact = MULTI_LANE_PATHS.get(pathname);
  if (exact) return exact;
  const prefixed = MULTI_LANE_PREFIXES.find(([prefix]) => pathname.startsWith(prefix));
  return prefixed?.[1] ?? [classifyPath(pathname)];
}

function recordOwnership(ownership, selected, pathname, kinds) {
  for (const kind of kinds) {
    ownership[kind].push(pathname);
    selected.add(kind);
  }
}

function selectsLane(selected, shared, kinds) {
  if (shared) return true;
  return kinds.some((kind) => selected.has(kind));
}

function classifyPaths(paths) {
  const ownership = { native: [], python: [], tooling: [], npm: [], docs: [], shared: [] };
  const selected = new Set();

  for (const pathname of paths) {
    const normalized = normalizePath(pathname);
    if (!normalized) continue;
    recordOwnership(ownership, selected, normalized, laneKinds(normalized));
  }

  if (selected.size === 0) {
    return { native: true, python: true, tooling: true, npm: true, docs: true, ownership };
  }
  const shared = selected.has('shared');
  return {
    native: selectsLane(selected, shared, ['native']),
    python: selectsLane(selected, shared, ['python', 'native']),
    tooling: selectsLane(selected, shared, ['tooling', 'native']),
    npm: selectsLane(selected, shared, ['npm']),
    docs: selectsLane(selected, shared, ['docs', 'python']),
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
  const { native, python, tooling, npm, docs, shared } = result.ownership;
  process.stdout.write(
    `native=${result.native}\npython=${result.python}\ntooling=${result.tooling}\n` +
      `npm=${result.npm}\ndocs=${result.docs}\n`
  );
  process.stderr.write(
    `CI ownership: native=${native.length}, python=${python.length}, ` +
      `tooling=${tooling.length}, npm=${npm.length}, docs=${docs.length}, ` +
      `shared=${shared.length}\n`
  );
}

module.exports = { classifyPath, classifyPaths };

if (require.main === module) main();
