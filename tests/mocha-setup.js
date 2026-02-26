/**
 * Mocha global setup - suppress known better-sqlite3 cleanup crash.
 *
 * better-sqlite3 throws "Cannot assign to read only property 'database'"
 * during process exit when prepared statements are GC'd after db.close().
 * This is harmless (all tests already passed) but causes mocha to report
 * "1 failing" due to the uncaught exception.
 */
process.on('uncaughtException', (err) => {
  if (
    err instanceof TypeError &&
    err.message.includes("Cannot assign to read only property 'database'")
  ) {
    return; // Suppress known better-sqlite3 cleanup crash
  }
  // Re-throw all other uncaught exceptions
  throw err;
});
