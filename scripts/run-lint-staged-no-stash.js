#!/usr/bin/env node

const { spawnSync } = require('child_process');
const fs = require('fs');
const path = require('path');

const PATCH_NAME = 'lint-staged_unstaged.patch';
const GIT_APPLY_ARGS = ['apply', '-v', '--whitespace=nowarn', '--recount', '--unidiff-zero'];

function run(command, args, options = {}) {
  return spawnSync(command, args, {
    cwd: process.cwd(),
    env: process.env,
    stdio: options.stdio || 'inherit',
    encoding: options.encoding,
  });
}

function resolveUnstagedPatchPath() {
  const result = run('git', ['rev-parse', '--git-path', PATCH_NAME], {
    stdio: 'pipe',
    encoding: 'utf8',
  });
  if (result.status !== 0) return null;
  const gitPath = result.stdout.trim();
  return path.isAbsolute(gitPath) ? gitPath : path.resolve(process.cwd(), gitPath);
}

function restoreUnstagedPatch(patchPath) {
  const direct = run('git', [...GIT_APPLY_ARGS, patchPath]);
  if (direct.status === 0) return true;

  const threeWay = run('git', [...GIT_APPLY_ARGS, '--3way', patchPath]);
  return threeWay.status === 0;
}

function main() {
  const patchPath = resolveUnstagedPatchPath();
  if (patchPath && fs.existsSync(patchPath)) {
    console.error(
      `ERROR: Refusing to overwrite unresolved unstaged changes at ${patchPath}.\n` +
        `Restore them with 'git apply "${patchPath}"', then remove the patch before committing.`
    );
    return 1;
  }

  const lintStagedBin = require.resolve('lint-staged/bin');
  const lintStaged = run(process.execPath, [lintStagedBin, '--no-stash', ...process.argv.slice(2)]);
  const lintStatus = lintStaged.status ?? 1;

  if (!patchPath || !fs.existsSync(patchPath)) {
    return lintStatus;
  }

  if (lintStatus === 0 || restoreUnstagedPatch(patchPath)) {
    fs.rmSync(patchPath, { force: true });
    return lintStatus;
  }

  console.error(
    `ERROR: lint-staged failed and unstaged changes conflicted with task edits.\n` +
      `The commit remains blocked. Your unstaged changes are preserved at ${patchPath}.\n` +
      `Resolve them with 'git apply --3way "${patchPath}"'; do not delete the patch first.`
  );
  return lintStatus;
}

process.exitCode = main();
