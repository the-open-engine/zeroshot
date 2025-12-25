/**
 * Test Setup Utilities
 *
 * Provides reusable test fixtures for zeroshot cluster tests:
 * - Temporary directory management
 * - Ledger initialization
 * - Orchestrator creation with isolation
 * - Resource cleanup
 *
 * Usage:
 *   const { createTestOrchestrator, cleanup } = require('./helpers/setup');
 *   let { orchestrator, tempDir, cleanup: cleanupFn } = createTestOrchestrator();
 *   // ... run tests ...
 *   await cleanupFn();
 */

const fs = require('fs');
const path = require('path');
const os = require('os');
const Ledger = require('../../src/ledger');
const MessageBus = require('../../src/message-bus');
const Orchestrator = require('../../src/orchestrator');

/**
 * Create a temporary directory
 * @param {string} prefix - Prefix for temp dir name
 * @returns {string} Path to created temp directory
 */
function createTempDir(prefix = 'zeroshot-test-') {
  return fs.mkdtempSync(path.join(os.tmpdir(), prefix));
}

/**
 * Create a Ledger and MessageBus for testing
 * @param {string} tempDir - Temporary directory for database
 * @returns {Object} { ledger, messageBus, dbPath }
 */
function createTestLedger(tempDir) {
  const dbPath = path.join(tempDir, 'test-ledger.db');
  const ledger = new Ledger(dbPath);
  const messageBus = new MessageBus(ledger);

  return { ledger, messageBus, dbPath };
}

/**
 * Create an Orchestrator with isolated storage for testing
 *
 * Initializes:
 * - Temporary storage directory (isolated from ~/.zeroshot)
 * - Orchestrator with quiet mode enabled
 * - skipLoad: true to avoid loading real cluster state
 *
 * @param {Object} options - Configuration options
 * @param {Function} options.taskRunner - Task runner instance (optional)
 * @returns {Object} { orchestrator, tempDir, cleanup }
 *
 * Example:
 *   const { orchestrator, tempDir, cleanup } = createTestOrchestrator();
 *   try {
 *     // ... run tests ...
 *   } finally {
 *     await cleanup();
 *   }
 */
function createTestOrchestrator(options = {}) {
  const tempDir = createTempDir('zeroshot-orchestrator-test-');

  const orchestrator = new Orchestrator({
    quiet: true,
    storageDir: tempDir,
    skipLoad: true,
    ...(options.taskRunner && { taskRunner: options.taskRunner }),
  });

  /**
   * Cleanup function: Stop all clusters and remove temp directory
   * @returns {Promise<void>}
   */
  const cleanupFn = async () => {
    try {
      // Stop all running clusters
      for (const cluster of orchestrator.listClusters()) {
        try {
          await orchestrator.stop(cluster.id);
        } catch {
          // Ignore errors during stop
        }
      }
    } catch {
      // Ignore errors during cleanup
    }

    // Remove temp directory
    if (tempDir && fs.existsSync(tempDir)) {
      try {
        fs.rmSync(tempDir, { recursive: true, force: true });
      } catch {
        // Ignore errors during directory removal
      }
    }
  };

  return { orchestrator, tempDir, cleanup: cleanupFn };
}

/**
 * Safely remove a temporary directory
 * @param {string} tempDir - Path to directory to remove
 */
function cleanup(tempDir) {
  if (tempDir && fs.existsSync(tempDir)) {
    try {
      fs.rmSync(tempDir, { recursive: true, force: true });
    } catch {
      // Ignore errors during cleanup
    }
  }
}

module.exports = {
  createTempDir,
  createTestLedger,
  createTestOrchestrator,
  cleanup,
};
