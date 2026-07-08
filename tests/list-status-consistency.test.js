/**
 * Regression test for issue #556: `zeroshot list --json` disagreeing with
 * `zeroshot status <id>` for a running cluster (phantom failed/msgs=0, WAL race).
 *
 * Root causes fixed (src/orchestrator.js, src/ledger.js, cli/index.js):
 *  - _loadClusters() used to mutate cluster.state='corrupted' as a side effect of
 *    a single 0-message read on every list/status call, with no grace period and
 *    no confirmation - any running cluster whose first message hadn't landed yet
 *    (or whose count() read raced a writer) got flipped and reported as failed.
 *  - getStatus()/listClusters() caught ledger read failures and fabricated
 *    messageCount=0, making a transient read failure indistinguishable from a
 *    genuinely empty cluster - the exact "msgs=0 $0" phantom from the bug report.
 *  - list/status opened a fresh read-write Ledger connection per call, taking a
 *    write lock and re-running schema DDL against a live daemon's connection.
 *
 * The first two sub-tests below are deterministic: they construct the exact
 * conditions that used to trigger each bug (no timing/race required) and fail
 * reliably against the pre-fix code. The final sub-test is a concurrency smoke
 * test that mirrors real CLI process behavior (fresh read-only Orchestrator
 * instances polling while a second connection writes).
 */

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const os = require('os');
const Orchestrator = require('../src/orchestrator.js');
const Ledger = require('../src/ledger.js');
const MockTaskRunner = require('./helpers/mock-task-runner.js');

function createTempDir() {
  const tmpBase = path.join(os.tmpdir(), 'zeroshot-test');
  if (!fs.existsSync(tmpBase)) {
    fs.mkdirSync(tmpBase, { recursive: true });
  }
  return fs.mkdtempSync(path.join(tmpBase, 'list-status-consistency-'));
}

function cleanupTempDir(tmpDir) {
  if (fs.existsSync(tmpDir)) {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function closeAllLedgers(orchestratorInstance) {
  for (const cluster of orchestratorInstance.clusters.values()) {
    try {
      cluster.ledger?.close();
    } catch {
      // Already closed - fine.
    }
  }
}

describe('list/status consistency (issue #556)', function () {
  this.timeout(20000);

  it('does not mark a running, 0-message cluster as corrupted merely from a load (list/status never mutates state as a side effect of a plain read)', async function () {
    const storageDir = createTempDir();
    const clusterId = 'zero-msg-running-1';
    const dbPath = path.join(storageDir, `${clusterId}.db`);

    // 0-message ledger: the real window between cluster creation and its
    // first ISSUE_OPENED message landing (or a transient read race) - not
    // itself evidence of corruption.
    const seedLedger = new Ledger(dbPath);
    seedLedger.close();

    fs.writeFileSync(
      path.join(storageDir, 'clusters.json'),
      JSON.stringify({
        [clusterId]: {
          id: clusterId,
          config: { agents: [] },
          state: 'running',
          createdAt: Date.now(),
          pid: process.pid, // our own test process - genuinely alive, not a zombie
        },
      })
    );

    const orchestrator = await Orchestrator.create({ storageDir, quiet: true });
    try {
      const status = orchestrator.getStatus(clusterId);
      assert.strictEqual(
        status.state,
        'running',
        `Expected state to remain "running" for a freshly-created 0-message cluster; ` +
          `got "${status.state}". The old unconditional 0-message heuristic in ` +
          `_loadClusters() must not run inside the list/status read path.`
      );

      const listEntry = orchestrator.listClusters().find((c) => c.id === clusterId);
      assert.ok(listEntry, 'listClusters() must not omit the cluster');
      assert.strictEqual(listEntry.state, 'running');
    } finally {
      orchestrator.close();
      cleanupTempDir(storageDir);
    }
  });

  it('does not fabricate messageCount=0 when a ledger read fails (surfaces as null instead)', async function () {
    const storageDir = createTempDir();
    const mockRunner = new MockTaskRunner();
    mockRunner.when('worker').returns({ done: true });

    const orchestrator = new Orchestrator({
      taskRunner: mockRunner,
      storageDir,
      skipLoad: true,
      quiet: true,
    });

    try {
      const config = {
        agents: [
          {
            id: 'worker',
            role: 'implementation',
            modelLevel: 'level2',
            outputFormat: 'text',
            triggers: [{ topic: 'ISSUE_OPENED', action: 'execute_task' }],
            prompt: 'You are a worker agent.',
          },
        ],
      };
      const { id: clusterId } = await orchestrator.start(config, { text: 'test issue' });
      const cluster = orchestrator.getCluster(clusterId);

      assert.ok(
        cluster.messageBus.count({ cluster_id: clusterId }) > 0,
        'sanity check: cluster should have messages before simulating a read failure'
      );

      // Simulate a transient ledger read failure (closed connection, corrupted
      // file, busy timeout, etc - any of which can happen mid-flight).
      cluster.ledger.close();

      const status = orchestrator.getStatus(clusterId);
      assert.notStrictEqual(
        status.messageCount,
        0,
        'A failed ledger read must not report messageCount=0 - that is indistinguishable ' +
          'from "confirmed empty" and is exactly the phantom "msgs=0 $0" from the bug report'
      );
      assert.strictEqual(
        status.messageCount,
        null,
        'Failed reads should surface as null, not a fabricated zero'
      );
    } finally {
      orchestrator.close();
      cleanupTempDir(storageDir);
    }
  });

  it('[concurrency smoke test] never reports a running cluster as failed/corrupted, omits it, or fabricates messageCount=0 while a writer is actively appending', async function () {
    const storageDir = createTempDir();
    const mockRunner = new MockTaskRunner();
    // Keep the worker "in flight" for the whole poll loop so the owning
    // orchestrator's cluster.state stays 'running' throughout the test.
    mockRunner.when('worker').delays(15000, { done: true });

    const owner = new Orchestrator({
      taskRunner: mockRunner,
      storageDir,
      skipLoad: true,
      quiet: true,
    });

    const config = {
      agents: [
        {
          id: 'worker',
          role: 'implementation',
          modelLevel: 'level2',
          outputFormat: 'text',
          triggers: [{ topic: 'ISSUE_OPENED', action: 'execute_task' }],
          prompt: 'You are a worker agent.',
        },
      ],
    };

    let writerLedger;
    let writerStopped = false;
    let writerLoop = Promise.resolve();

    try {
      const { id: clusterId } = await owner.start(config, { text: 'test issue' });
      const dbPath = path.join(storageDir, `${clusterId}.db`);

      // Concurrent writer on a second connection, independent of the read-only
      // pollers below - simulates a live daemon appending progress messages.
      writerLedger = new Ledger(dbPath);
      let appended = 0;
      writerLoop = (async () => {
        while (!writerStopped) {
          writerLedger.append({
            cluster_id: clusterId,
            topic: 'WORKER_PROGRESS',
            sender: 'worker',
            receiver: 'broadcast',
            content: { text: `progress ${appended}` },
          });
          appended += 1;
          await sleep(10);
        }
      })();

      const violations = [];
      let sawNonZero = false;

      for (let i = 0; i < 40; i++) {
        // Mirror real CLI process behavior: `zeroshot list` and `zeroshot status`
        // each construct their own fresh, read-only Orchestrator instance.
        const [listOrch, statusOrch] = await Promise.all([
          Orchestrator.create({ storageDir, quiet: true, readonly: true }),
          Orchestrator.create({ storageDir, quiet: true, readonly: true }),
        ]);

        const listEntry = listOrch.listClusters().find((c) => c.id === clusterId);
        let status = null;
        try {
          status = statusOrch.getStatus(clusterId);
        } catch (error) {
          violations.push(`iteration ${i}: status threw: ${error.message}`);
        }

        if (!listEntry) {
          violations.push(`iteration ${i}: list omitted a live running cluster entirely`);
        } else {
          if (status && status.messageCount > 0) {
            sawNonZero = true;
          }

          if (sawNonZero) {
            if (listEntry.messageCount === 0) {
              violations.push(
                `iteration ${i}: list reported messageCount=0 after messages were already observed`
              );
            }
            if (status && status.messageCount === 0) {
              violations.push(
                `iteration ${i}: status reported messageCount=0 after messages were already observed`
              );
            }
          }

          if (listEntry.state !== 'running') {
            violations.push(
              `iteration ${i}: list reported state="${listEntry.state}", expected "running"`
            );
          }
          if (status && status.state !== 'running') {
            violations.push(
              `iteration ${i}: status reported state="${status.state}", expected "running"`
            );
          }
        }

        closeAllLedgers(listOrch);
        closeAllLedgers(statusOrch);

        await sleep(15);
      }

      assert.deepStrictEqual(
        violations,
        [],
        `list/status disagreed with reality (${violations.length} violation(s)):\n${violations.join('\n')}`
      );
    } finally {
      writerStopped = true;
      await writerLoop;
      try {
        writerLedger?.close();
      } catch {
        // Already closed - fine.
      }
      owner.close();
      cleanupTempDir(storageDir);
    }
  });
});
