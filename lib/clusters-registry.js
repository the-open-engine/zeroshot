/**
 * Single read/write path for clusters.json.
 *
 * All callers that need the raw registry (Orchestrator, id-detector, gc,
 * socket-discovery, CLI) go through readClustersFileSync/writeClustersFileAtomic
 * instead of ad-hoc JSON.parse(fs.readFileSync(...)) / fs.writeFileSync(...).
 *
 * Writes are atomic (temp file + rename) so a reader can never observe a
 * partially-written file, even without taking the write lock. Callers that
 * read-modify-write (Orchestrator._saveClusters) still need proper-lockfile
 * around the whole operation to avoid losing concurrent updates.
 */

const fs = require('fs');
const path = require('path');

function clustersFilePath(storageDir) {
  return path.join(storageDir, 'clusters.json');
}

/**
 * Read clusters.json. Returns {} if missing or unparsable.
 * @param {string} storageDir
 * @returns {Object}
 */
function readClustersFileSync(storageDir) {
  const clustersFile = clustersFilePath(storageDir);
  if (!fs.existsSync(clustersFile)) {
    return {};
  }
  const raw = fs.readFileSync(clustersFile, 'utf8');
  return JSON.parse(raw);
}

/**
 * Write clusters.json atomically (write to a pid-scoped temp file, then rename).
 * rename(2) is atomic on the same filesystem, so no reader can observe a partial file.
 * @param {string} storageDir
 * @param {Object} data
 */
function writeClustersFileAtomic(storageDir, data) {
  const clustersFile = clustersFilePath(storageDir);
  const tmpPath = `${clustersFile}.tmp-${process.pid}`;
  fs.writeFileSync(tmpPath, JSON.stringify(data, null, 2));
  fs.renameSync(tmpPath, clustersFile);
}

module.exports = { clustersFilePath, readClustersFileSync, writeClustersFileAtomic };
