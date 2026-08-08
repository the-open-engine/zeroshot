const fs = require('fs');
const path = require('path');

const CONTAINMENT_ERROR_CODE = 'ERR_COPY_CONTAINMENT';

class CopyContainmentError extends Error {
  constructor(relativePath, reason) {
    super(`Copy containment violation for ${JSON.stringify(relativePath)}: ${reason}`);
    this.name = 'CopyContainmentError';
    this.code = CONTAINMENT_ERROR_CODE;
    this.relativePath = relativePath;
  }
}

function isContained(rootPath, targetPath) {
  const relative = path.relative(rootPath, targetPath);
  return (
    relative === '' ||
    (!path.isAbsolute(relative) && relative !== '..' && !relative.startsWith(`..${path.sep}`))
  );
}

function containmentError(relativePath, reason, cause) {
  const error = new CopyContainmentError(relativePath, reason);
  if (cause) {
    error.cause = cause;
  }
  return error;
}

function statIdentity(targetPath) {
  const stats = fs.statSync(targetPath, { bigint: true });
  return {
    device: stats.dev.toString(),
    inode: stats.ino.toString(),
    directory: stats.isDirectory(),
  };
}

function pinRoot(rootPath, label, expectedRoot) {
  const requestedPath = path.resolve(rootPath);
  const canonicalPath = fs.realpathSync.native(requestedPath);
  const identity = statIdentity(canonicalPath);

  if (!identity.directory) {
    throw containmentError('', `${label} root is not a directory`);
  }

  const pinnedRoot = {
    requestedPath,
    canonicalPath,
    device: identity.device,
    inode: identity.inode,
  };

  if (
    expectedRoot &&
    (expectedRoot.canonicalPath !== pinnedRoot.canonicalPath ||
      expectedRoot.device !== pinnedRoot.device ||
      expectedRoot.inode !== pinnedRoot.inode)
  ) {
    throw containmentError('', `${label} root changed after it was pinned`);
  }

  return pinnedRoot;
}

function assertPinnedRoot(root, label, relativePath) {
  let identity;
  try {
    identity = statIdentity(root.canonicalPath);
  } catch (err) {
    throw containmentError(relativePath, `${label} root can no longer be resolved`, err);
  }

  if (identity.device !== root.device || identity.inode !== root.inode || !identity.directory) {
    throw containmentError(relativePath, `${label} root changed after it was pinned`);
  }
}

function validateRelativePath(relativePath, pathApi = path) {
  if (typeof relativePath !== 'string' || relativePath.length === 0) {
    throw containmentError(relativePath, 'path must be a non-empty relative string');
  }
  if (relativePath.includes('\0')) {
    throw containmentError(relativePath, 'path contains a null byte');
  }
  if (pathApi.isAbsolute(relativePath) || pathApi.parse(relativePath).root) {
    throw containmentError(relativePath, 'absolute paths are not allowed');
  }

  const components =
    pathApi.sep === '\\' ? relativePath.split(/[\\/]/) : relativePath.split(pathApi.sep);
  if (components.some((component) => component === '' || component === '.' || component === '..')) {
    throw containmentError(
      relativePath,
      'empty, current-directory, and traversal components are not allowed'
    );
  }

  return pathApi.normalize(relativePath);
}

function resolveSourcePath(boundary, relativePath) {
  const normalizedPath = validateRelativePath(relativePath);
  const root = boundary.sourceRoot;
  assertPinnedRoot(root, 'source', relativePath);

  const candidatePath = path.resolve(root.canonicalPath, normalizedPath);
  if (!isContained(root.canonicalPath, candidatePath)) {
    throw containmentError(relativePath, 'source path escapes its pinned root');
  }

  let canonicalPath;
  try {
    canonicalPath = fs.realpathSync.native(candidatePath);
  } catch (err) {
    if (err.code === 'ELOOP') {
      throw containmentError(relativePath, 'source path contains a symlink cycle', err);
    }
    throw err;
  }

  if (!isContained(root.canonicalPath, canonicalPath)) {
    throw containmentError(relativePath, 'resolved source path escapes its pinned root');
  }
  return canonicalPath;
}

function resolveDestinationPath(boundary, relativePath) {
  const normalizedPath = validateRelativePath(relativePath);
  const root = boundary.destinationRoot;
  assertPinnedRoot(root, 'destination', relativePath);

  const candidatePath = path.resolve(root.canonicalPath, normalizedPath);
  if (!isContained(root.canonicalPath, candidatePath)) {
    throw containmentError(relativePath, 'destination path escapes its pinned root');
  }

  let existingPath;
  try {
    fs.lstatSync(candidatePath);
    existingPath = candidatePath;
  } catch (err) {
    if (err.code !== 'ENOENT') {
      throw err;
    }
    // The copy pipeline creates directories parent-first in phase two, so the
    // immediate parent must exist before any mkdir/copy effect is attempted.
    existingPath = path.dirname(candidatePath);
  }

  let canonicalExistingPath;
  try {
    canonicalExistingPath = fs.realpathSync.native(existingPath);
  } catch (err) {
    throw containmentError(relativePath, 'destination contains an unresolved symlink', err);
  }

  if (!isContained(root.canonicalPath, canonicalExistingPath)) {
    throw containmentError(relativePath, 'resolved destination path escapes its pinned root');
  }

  const unresolvedSuffix = path.relative(existingPath, candidatePath);
  const resolvedPath = unresolvedSuffix
    ? path.join(canonicalExistingPath, unresolvedSuffix)
    : canonicalExistingPath;
  if (!isContained(root.canonicalPath, resolvedPath)) {
    throw containmentError(relativePath, 'resolved destination path escapes its pinned root');
  }
  return resolvedPath;
}

function createCopyBoundary(sourceBase, destinationBase, expectedBoundary) {
  return {
    sourceRoot: pinRoot(sourceBase, 'source', expectedBoundary?.sourceRoot),
    destinationRoot: pinRoot(destinationBase, 'destination', expectedBoundary?.destinationRoot),
  };
}

function resolveCopyPath(boundary, relativePath) {
  return {
    sourcePath: resolveSourcePath(boundary, relativePath),
    destinationPath: resolveDestinationPath(boundary, relativePath),
  };
}

function isCopyContainmentError(error) {
  return error?.code === CONTAINMENT_ERROR_CODE;
}

function copyErrorFromPayload(payload) {
  const error =
    payload.code === CONTAINMENT_ERROR_CODE
      ? new CopyContainmentError(payload.relativePath, 'worker rejected an unsafe path')
      : new Error(payload.message);
  error.name = payload.name || error.name;
  error.message = payload.message;
  if (payload.code) {
    error.code = payload.code;
  }
  return error;
}

module.exports = {
  CONTAINMENT_ERROR_CODE,
  CopyContainmentError,
  copyErrorFromPayload,
  createCopyBoundary,
  isCopyContainmentError,
  resolveCopyPath,
  resolveSourcePath,
  validateRelativePath,
};
