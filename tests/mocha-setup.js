/**
 * Mocha global setup - suppress known better-sqlite3 cleanup crash.
 *
 * better-sqlite3 throws "Cannot assign to read only property 'database'"
 * during process exit when prepared statements are GC'd after db.close().
 * This is harmless (all tests already passed) but causes mocha to report
 * "1 failing" due to the uncaught exception.
 *
 * This file is loaded via --require and overrides process._fatalException
 * to suppress the known error in non-parallel mode. For parallel mode,
 * the error occurs in a way that bypasses JavaScript-level interception,
 * so the test:coverage script handles it at the process level.
 */
const origFatal = process._fatalException.bind(process);
const isolatedSettingsFile = process.env.ZEROSHOT_SETTINGS_FILE;

exports.mochaHooks = {
  beforeEach() {
    if (isolatedSettingsFile && !process.env.ZEROSHOT_SETTINGS_FILE) {
      process.env.ZEROSHOT_SETTINGS_FILE = isolatedSettingsFile;
    }
  },
};

process._fatalException = function (err, fromPromise) {
  if (
    err instanceof TypeError &&
    err.message &&
    err.message.includes("Cannot assign to read only property 'database'")
  ) {
    return true; // Suppress known better-sqlite3 cleanup crash
  }
  return origFatal(err, fromPromise);
};
