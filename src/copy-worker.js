/**
 * Copy Worker - Worker thread for parallel file copying
 *
 * Handles copying a batch of files from source to destination.
 * Used by IsolationManager._copyDirExcluding() for parallel copying.
 */

const { parentPort, workerData } = require('worker_threads');
const fs = require('fs');
const path = require('path');

const { files, sourceBase, destBase } = workerData;

let copied = 0;
let skipped = 0;
const errors = [];
const resolvedDestBase = path.resolve(destBase);

for (const relativePath of files) {
  const srcPath = path.join(sourceBase, relativePath);
  const destPath = path.resolve(destBase, relativePath);

  if (destPath !== resolvedDestBase && !destPath.startsWith(resolvedDestBase + path.sep)) {
    skipped++;
    errors.push({ file: relativePath, error: 'Invalid destination path' });
    continue;
  }

  try {
    // Ensure parent directory exists
    const destDir = path.dirname(destPath);
    if (!fs.existsSync(destDir)) {
      fs.mkdirSync(destDir, { recursive: true });
    }

    // Copy the file
    fs.copyFileSync(srcPath, destPath);
    copied++;
  } catch (err) {
    // Skip files we can't copy (permission denied, broken symlinks, etc.)
    if (err.code === 'EACCES' || err.code === 'EPERM' || err.code === 'ENOENT') {
      skipped++;
      continue;
    }
    errors.push({ file: relativePath, error: err.message });
  }
}

// Report results back to main thread
parentPort.postMessage({ copied, skipped, errors });
