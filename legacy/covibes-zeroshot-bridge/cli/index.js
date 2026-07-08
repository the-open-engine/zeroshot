#!/usr/bin/env node

const childProcess = require('child_process');
const fs = require('fs');
const path = require('path');

const LEGACY_PACKAGE_NAME = '@covibes/zeroshot';
const NEW_PACKAGE_NAME = '@the-open-engine/zeroshot';
const NEW_PACKAGE_SPEC = `${NEW_PACKAGE_NAME}@latest`;

function printNotice() {
  console.error(
    `\n⚠️  ${LEGACY_PACKAGE_NAME} has moved to ${NEW_PACKAGE_NAME}. ` +
      'Run `zeroshot update` to switch this installation.\n'
  );
}

function hasPathSuffix(parts, suffix) {
  if (suffix.length > parts.length) {
    return false;
  }

  const start = parts.length - suffix.length;
  return suffix.every((part, index) => parts[start + index] === part);
}

function joinPathParts(parts) {
  const joined = parts.join(path.sep);
  return joined === '' ? path.parse(process.cwd()).root : joined;
}

function deriveInstallPrefixFromPackageRoot(packageRoot, packageName = LEGACY_PACKAGE_NAME) {
  const parts = path.resolve(packageRoot).split(path.sep);
  const packageParts = packageName.split('/');

  if (!hasPathSuffix(parts, packageParts)) {
    return null;
  }

  const nodeModulesIndex = parts.length - packageParts.length - 1;
  if (nodeModulesIndex < 0 || parts[nodeModulesIndex] !== 'node_modules') {
    return null;
  }

  if (parts[nodeModulesIndex - 1] === 'lib') {
    return joinPathParts(parts.slice(0, nodeModulesIndex - 1));
  }

  return joinPathParts(parts.slice(0, nodeModulesIndex));
}

function getPackageRoot() {
  return path.dirname(path.dirname(__filename));
}

function resolveNpmCommand(installPrefix) {
  const npmName = process.platform === 'win32' ? 'npm.cmd' : 'npm';
  const candidates = [];

  if (installPrefix) {
    candidates.push(path.join(installPrefix, 'bin', npmName));
  }

  candidates.push(path.join(path.dirname(process.execPath), npmName));

  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }

  return npmName;
}

function getInstallPrefix(packageRoot = getPackageRoot()) {
  const derivedPrefix = deriveInstallPrefixFromPackageRoot(packageRoot);
  if (derivedPrefix) {
    return derivedPrefix;
  }

  return childProcess
    .execFileSync(resolveNpmCommand(null), ['config', 'get', 'prefix'], { encoding: 'utf8' })
    .trim();
}

function buildInstallArgs(installPrefix) {
  return ['install', '-g', '--prefix', installPrefix, '--force', NEW_PACKAGE_SPEC];
}

function runUpdate(options = {}) {
  const installPrefix = options.installPrefix || getInstallPrefix(options.packageRoot);
  const npmCommand = options.npmCommand || resolveNpmCommand(installPrefix);
  const spawn = options.spawn || childProcess.spawn;

  console.error(`Installing ${NEW_PACKAGE_NAME} into ${installPrefix}...`);

  return new Promise((resolve) => {
    const proc = spawn(npmCommand, buildInstallArgs(installPrefix), {
      stdio: 'inherit',
      shell: false,
    });

    proc.on('close', (code) => {
      if (code === 0) {
        console.error(`\nDone. ${NEW_PACKAGE_NAME} now owns the global zeroshot command.\n`);
        resolve(true);
        return;
      }

      console.error(`\nUpdate failed. Run manually: npm install -g --force ${NEW_PACKAGE_SPEC}\n`);
      resolve(false);
    });

    proc.on('error', () => {
      console.error(`\nUpdate failed. Run manually: npm install -g --force ${NEW_PACKAGE_SPEC}\n`);
      resolve(false);
    });
  });
}

function resolveNewCli() {
  return require.resolve(`${NEW_PACKAGE_NAME}/cli/index.js`);
}

function delegateToNewCli(args, options = {}) {
  const spawn = options.spawn || childProcess.spawn;
  const cliPath = options.cliPath || resolveNewCli();

  const proc = spawn(process.execPath, [cliPath, ...args], {
    stdio: 'inherit',
    shell: false,
  });

  proc.on('close', (code) => {
    process.exit(code ?? 1);
  });

  proc.on('error', () => {
    console.error(`Could not start ${NEW_PACKAGE_NAME}. Run: zeroshot update`);
    process.exit(1);
  });
}

async function main(args = process.argv.slice(2)) {
  printNotice();

  if (args[0] === 'update') {
    const success = await runUpdate();
    process.exit(success ? 0 : 1);
    return;
  }

  delegateToNewCli(args);
}

if (require.main === module) {
  main();
}

module.exports = {
  LEGACY_PACKAGE_NAME,
  NEW_PACKAGE_NAME,
  NEW_PACKAGE_SPEC,
  printNotice,
  deriveInstallPrefixFromPackageRoot,
  getInstallPrefix,
  buildInstallArgs,
  runUpdate,
  delegateToNewCli,
};
