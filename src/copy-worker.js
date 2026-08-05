/**
 * Copy Worker - Worker thread for parallel file copying
 *
 * Handles copying a batch of files from source to destination.
 * Used by IsolationManager._copyDirExcluding() for parallel copying.
 */

const { parentPort, workerData } = require('worker_threads');
const fs = require('fs');
const path = require('path');
const {
  createCopyBoundary,
  isCopyContainmentError,
  resolveCopyPath,
} = require('./copy-containment');

const { files, sourceBase, destBase, expectedBoundary } = workerData;
const copyBoundary = createCopyBoundary(sourceBase, destBase, expectedBoundary);

let copied = 0;
let skipped = 0;
let error = null;

for (const relativePath of files) {
  try {
    // Ensure parent directory exists
    const parentRelativePath = path.dirname(relativePath);
    if (parentRelativePath !== '.') {
      const { destinationPath: destDir } = resolveCopyPath(copyBoundary, parentRelativePath);
      if (!fs.existsSync(destDir)) {
        fs.mkdirSync(destDir, { recursive: true });
      }
    }

    // Copy the file
    const { sourcePath, destinationPath } = resolveCopyPath(copyBoundary, relativePath);
    fs.copyFileSync(sourcePath, destinationPath);
    copied++;
  } catch (err) {
    // Skip files we can't copy (permission denied, broken symlinks, etc.)
    if (
      !isCopyContainmentError(err) &&
      (err.code === 'EACCES' || err.code === 'EPERM' || err.code === 'ENOENT')
    ) {
      skipped++;
      continue;
    }
    error = {
      file: relativePath,
      name: err.name,
      code: err.code,
      message: err.message,
    };
    break;
  }
}

// Report results back to main thread
parentPort.postMessage({ copied, skipped, error });
