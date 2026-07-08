/**
 * Detects whether the npm global bin directory is on PATH, so we can warn
 * the user when `npm install -g` succeeds but the `zeroshot` binary is
 * unreachable (e.g. non-standard Node installs whose global bin dir isn't
 * exported).
 */

const path = require('path');

function getGlobalBinDir(installPrefix) {
  if (process.platform === 'win32') {
    return installPrefix;
  }

  return path.join(installPrefix, 'bin');
}

function isDirOnPath(dir, pathEnv = process.env.PATH || '') {
  const resolvedDir = path.resolve(dir);

  return pathEnv
    .split(path.delimiter)
    .filter((entry) => entry.length > 0)
    .some((entry) => path.resolve(entry) === resolvedDir);
}

function getPathExportLine(dir) {
  return `export PATH="${dir}:$PATH"`;
}

function checkBinDirOnPath(options = {}) {
  if (process.platform === 'win32') {
    return { onPath: true, binDir: null };
  }

  try {
    const { getInstallPrefix } = require('../cli/lib/update-checker');
    const installPrefix = options.installPrefix || getInstallPrefix(options);
    const binDir = getGlobalBinDir(installPrefix);

    return { onPath: isDirOnPath(binDir, options.pathEnv), binDir };
  } catch {
    return { onPath: true, binDir: null };
  }
}

function printPathWarning(binDir) {
  console.error(
    `[zeroshot] Warning: ${binDir} is not on your PATH — the 'zeroshot' command may not be found.`
  );
  console.error(`  ${getPathExportLine(binDir)}`);
  console.error(
    '  Add this line to your shell profile (~/.zshrc, ~/.bashrc, or ~/.profile) to fix this permanently.'
  );
}

module.exports = {
  getGlobalBinDir,
  isDirOnPath,
  getPathExportLine,
  checkBinDirOnPath,
  printPathWarning,
};
