const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const Database = require('better-sqlite3');

const { buildRunSummary, listRunSummaries } = require('../../src/run-catalog');

function createTempStorageDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-run-catalog-'));
}

function cleanupDir(dirPath) {
  fs.rmSync(dirPath, { recursive: true, force: true });
}

function createMessagesDb(storageDir, clusterId, rows) {
  const dbPath = path.join(storageDir, `${clusterId}.db`);
  const db = new Database(dbPath);
  try {
    db.exec(`
      CREATE TABLE messages (
        id TEXT PRIMARY KEY,
        timestamp INTEGER NOT NULL,
        topic TEXT NOT NULL,
        sender TEXT NOT NULL,
        receiver TEXT NOT NULL,
        content_text TEXT,
        content_data TEXT,
        metadata TEXT,
        cluster_id TEXT NOT NULL
      );
    `);

    const insert = db.prepare(`
      INSERT INTO messages (id, timestamp, topic, sender, receiver, content_text, content_data, metadata, cluster_id)
      VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
    `);

    for (const row of rows) {
      insert.run(
        row.id,
        row.timestamp,
        row.topic,
        row.sender,
        row.receiver ?? 'broadcast',
        row.contentText ?? null,
        row.contentData ? JSON.stringify(row.contentData) : null,
        null,
        clusterId
      );
    }
  } finally {
    db.close();
  }
}

function createNonClusterDb(storageDir, fileName) {
  const dbPath = path.join(storageDir, fileName);
  const db = new Database(dbPath);
  try {
    db.exec(`
      CREATE TABLE runs (
        id TEXT PRIMARY KEY,
        created_at INTEGER NOT NULL
      );
    `);
  } finally {
    db.close();
  }
}

describe('run-catalog', function () {
  it('classifies completed history runs and extracts task summary', function () {
    const storageDir = createTempStorageDir();
    try {
      createMessagesDb(storageDir, 'infinite-surge-84', [
        {
          id: 'msg-1',
          timestamp: 1773327544000,
          topic: 'ISSUE_OPENED',
          sender: 'system',
          contentText: '# GitHub Issue #1672\n\n## Title\nKeep only two Next.js templates',
          contentData: { issue_number: 1672, title: 'Keep only two Next.js templates' },
        },
        {
          id: 'msg-2',
          timestamp: 1773335351000,
          topic: 'CLUSTER_COMPLETE',
          sender: 'git-pusher',
          contentText: 'done',
        },
      ]);

      const summary = buildRunSummary({
        clusterId: 'infinite-surge-84',
        storageDir,
      });

      assert(summary);
      assert.strictEqual(summary.state, 'completed');
      assert.strictEqual(summary.issue, 1672);
      assert.strictEqual(summary.taskSummary, 'Keep only two Next.js templates');
      assert.strictEqual(summary.messageCount, 2);
      assert.strictEqual(summary.orphaned, false);
    } finally {
      cleanupDir(storageDir);
    }
  });

  it('maps stopped registry clusters with failure messages to failed and marks orphaned daemons', function () {
    const storageDir = createTempStorageDir();
    try {
      createMessagesDb(storageDir, 'scarlet-sphinx-73', [
        {
          id: 'msg-1',
          timestamp: 1773313422000,
          topic: 'ISSUE_OPENED',
          sender: 'system',
          contentText: '# Manual Input\n\nEliminate all current robustness-engine violations.',
          contentData: { title: 'Eliminate all current robustness-engine violations.' },
        },
        {
          id: 'msg-2',
          timestamp: 1773315107000,
          topic: 'AGENT_ERROR',
          sender: 'planner',
          contentText: 'Hook execution failed after 3 attempts.',
        },
      ]);

      const summary = buildRunSummary({
        clusterId: 'scarlet-sphinx-73',
        storageDir,
        registryEntry: {
          state: 'stopped',
          createdAt: 1773313422000,
          daemonPid: 4242,
          issue: null,
          agentStates: [{ id: 'planner', state: 'error', currentTask: false }],
        },
        isProcessRunning: (pid) => pid === 4242,
      });

      assert(summary);
      assert.strictEqual(summary.state, 'failed');
      assert.strictEqual(summary.orphaned, true);
      assert.strictEqual(summary.currentAgent, 'planner');
    } finally {
      cleanupDir(storageDir);
    }
  });

  it('surfaces setup_failed runs from daemon logs even when no messages exist', function () {
    const storageDir = createTempStorageDir();
    try {
      fs.writeFileSync(
        path.join(storageDir, 'fierce-canyon-56-daemon.log'),
        [
          'Provider override: claude (all agents)',
          './scripts/setup-worktree.sh: not found',
          'Error: Command failed: ./scripts/setup-worktree.sh',
        ].join('\n'),
        'utf8'
      );

      const summary = buildRunSummary({
        clusterId: 'fierce-canyon-56',
        storageDir,
      });

      assert(summary);
      assert.strictEqual(summary.state, 'setup_failed');
      assert.match(summary.failureReason, /Command failed/);
      assert.strictEqual(summary.messageCount, 0);
    } finally {
      cleanupDir(storageDir);
    }
  });

  it('filters active runs via listRunSummaries', function () {
    const storageDir = createTempStorageDir();
    try {
      createMessagesDb(storageDir, 'electric-galaxy-25', [
        {
          id: 'msg-1',
          timestamp: Date.now() - 1000,
          topic: 'ISSUE_OPENED',
          sender: 'system',
          contentText: '# Manual Input\n\nFix it',
        },
      ]);
      fs.writeFileSync(
        path.join(storageDir, 'clusters.json'),
        JSON.stringify({
          'electric-galaxy-25': {
            id: 'electric-galaxy-25',
            state: 'running',
            createdAt: Date.now() - 1000,
            daemonPid: 7777,
            issue: null,
            agentStates: [{ id: 'fixer', state: 'executing_task', currentTask: true }],
          },
        }),
        'utf8'
      );

      const runs = listRunSummaries({
        storageDir,
        activeOnly: true,
        isProcessRunning: (pid) => pid === 7777,
      });

      assert.strictEqual(runs.length, 1);
      assert.strictEqual(runs[0].id, 'electric-galaxy-25');
      assert.strictEqual(runs[0].state, 'running');
    } finally {
      cleanupDir(storageDir);
    }
  });

  it('ignores sqlite files that do not contain cluster messages', function () {
    const storageDir = createTempStorageDir();
    try {
      createMessagesDb(storageDir, 'electric-galaxy-25', [
        {
          id: 'msg-1',
          timestamp: Date.now() - 1000,
          topic: 'ISSUE_OPENED',
          sender: 'system',
          contentText: '# Manual Input\n\nFix it',
        },
      ]);
      createNonClusterDb(storageDir, 'runs.db');

      const runs = listRunSummaries({
        storageDir,
        isProcessRunning: () => false,
      });

      assert.strictEqual(runs.length, 1);
      assert.strictEqual(runs[0].id, 'electric-galaxy-25');
    } finally {
      cleanupDir(storageDir);
    }
  });

  it('prefers live running registry state over historical failure markers after resume', function () {
    const storageDir = createTempStorageDir();
    try {
      createMessagesDb(storageDir, 'electric-galaxy-25', [
        {
          id: 'msg-1',
          timestamp: 1773313422000,
          topic: 'ISSUE_OPENED',
          sender: 'system',
          contentText: '# Manual Input\n\nEliminate all current robustness-engine violations.',
        },
        {
          id: 'msg-2',
          timestamp: 1773315107000,
          topic: 'AGENT_ERROR',
          sender: 'fixer',
          contentText: 'Task violet-aegis-73 polling failed after 30 consecutive failures',
        },
        {
          id: 'msg-3',
          timestamp: 1773332772000,
          topic: 'AGENT_LIFECYCLE',
          sender: 'fixer',
          contentText: 'fixer: TASK_STARTED',
        },
      ]);

      const summary = buildRunSummary({
        clusterId: 'electric-galaxy-25',
        storageDir,
        registryEntry: {
          state: 'running',
          createdAt: 1773313422000,
          daemonPid: 7777,
          issue: null,
          agentStates: [{ id: 'fixer', state: 'executing_task', currentTask: true }],
        },
        isProcessRunning: (pid) => pid === 7777,
      });

      assert(summary);
      assert.strictEqual(summary.state, 'running');
      assert.strictEqual(summary.currentAgent, 'fixer');
      assert.strictEqual(summary.orphaned, false);
    } finally {
      cleanupDir(storageDir);
    }
  });

  it('falls back to registry metadata when SQLite runtime is unavailable', function () {
    const storageDir = createTempStorageDir();
    try {
      fs.writeFileSync(path.join(storageDir, 'runtime-fracture-12.db'), '');

      const summary = buildRunSummary({
        clusterId: 'runtime-fracture-12',
        storageDir,
        registryEntry: {
          state: 'running',
          createdAt: 1773313422000,
          daemonPid: 5151,
          issue: 1234,
          agentStates: [{ id: 'planner', state: 'executing_task', currentTask: true }],
        },
        isProcessRunning: (pid) => pid === 5151,
        readDbHistoryFn: () => ({
          sqliteUnavailable: true,
          sqliteWarning:
            'SQLite runtime unavailable for read-only run history. Rebuild SQLite bindings.',
        }),
      });

      assert(summary);
      assert.strictEqual(summary.state, 'running');
      assert.strictEqual(summary.issue, 1234);
      assert.strictEqual(summary.currentAgent, 'planner');
      assert.strictEqual(summary.messageCount, null);
      assert.match(summary.warning, /SQLite runtime unavailable/);
    } finally {
      cleanupDir(storageDir);
    }
  });

  it('falls back to setup log summary when SQLite runtime is unavailable without registry data', function () {
    const storageDir = createTempStorageDir();
    try {
      fs.writeFileSync(path.join(storageDir, 'runtime-fracture-13.db'), '');
      fs.writeFileSync(
        path.join(storageDir, 'runtime-fracture-13-daemon.log'),
        'Error: Command failed: ./scripts/setup-worktree.sh',
        'utf8'
      );

      const summary = buildRunSummary({
        clusterId: 'runtime-fracture-13',
        storageDir,
        readDbHistoryFn: () => ({
          sqliteUnavailable: true,
          sqliteWarning:
            'SQLite runtime unavailable for read-only run history. Rebuild SQLite bindings.',
        }),
      });

      assert(summary);
      assert.strictEqual(summary.state, 'setup_failed');
      assert.strictEqual(summary.messageCount, null);
      assert.match(summary.warning, /SQLite runtime unavailable/);
      assert.match(summary.failureReason, /Command failed/);
    } finally {
      cleanupDir(storageDir);
    }
  });
});
