/**
 * Suppress better-sqlite3 teardown error in parallel test workers.
 *
 * When mocha parallel workers exit, Node.js GC finalizes better-sqlite3
 * Database objects. The native finalizer iterates cached Statement objects
 * via Array.forEach and attempts to set their `.database` property, which
 * is read-only after the DB is already closed. This produces:
 *
 *   TypeError: Cannot assign to read only property 'database' of object '#<Statement>'
 *
 * This is harmless — the DB is already closed, the process is exiting.
 * Mocha reports it as "Uncaught error outside test suite" and fails the run.
 *
 * Loaded via mocha --require. Exports root hooks for parallel mode support.
 */

// Handler for uncaught exceptions (works in both serial and parallel modes)
process.on('uncaughtException', (err) => {
  if (
    err instanceof TypeError &&
    err.message.includes("Cannot assign to read only property 'database'")
  ) {
    // Harmless better-sqlite3 teardown race — suppress
    return;
  }
  // Re-throw everything else — use console.error + exit since re-throwing
  // from uncaughtException handler is unreliable
  console.error(err);
  process.exit(1);
});

// Root hooks for mocha parallel mode (run in each worker process)
module.exports = {
  mochaHooks: {
    afterAll() {
      // Force garbage collection of sqlite objects BEFORE mocha worker exits.
      // This prevents the native finalizer from running during exit teardown.
      if (global.gc) {
        global.gc();
      }
    },
  },
};
