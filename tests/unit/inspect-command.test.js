const assert = require('assert');

const {
  buildClusterInspection,
  buildTaskInspection,
  inferActivity,
  summarizeTaskRecord,
} = require('../../cli/commands/inspect');

describe('inspect command helpers', function () {
  it('should flag stale running task records and missing logs', function () {
    const now = Date.parse('2026-03-08T22:20:00.000Z');
    const task = summarizeTaskRecord(
      {
        id: 'task-123',
        status: 'running',
        pid: 4242,
        exitCode: null,
        error: null,
        cwd: '/tmp/work',
        sessionId: null,
        attachable: true,
        socketPath: '/tmp/task.sock',
        logFile: '/tmp/task.log',
        createdAt: '2026-03-08T18:52:01.000Z',
        updatedAt: '2026-03-08T18:52:01.000Z',
      },
      now,
      () => false
    );

    assert.strictEqual(task.logFileExists, false);
    assert.strictEqual(task.socketPathExists, false);
    assert.ok(task.updatedAgeMs > 0);
    assert(task.warnings.some((warning) => warning.includes('task log file missing')));
    assert(task.warnings.some((warning) => warning.includes('task record stale')));
  });

  it('should classify active and stuck process activity', function () {
    assert.strictEqual(
      inferActivity(
        {
          exists: true,
          cpuPercent: 12,
          state: 'S',
          childCount: 0,
          network: { hasActivity: false },
        },
        null
      ),
      'cpu-active'
    );

    assert.strictEqual(
      inferActivity(
        {
          exists: true,
          cpuPercent: 0.1,
          state: 'S',
          childCount: 0,
          network: { hasActivity: false },
        },
        { isLikelyStuck: true }
      ),
      'likely-stuck'
    );
  });

  it('should build cluster inspection with agent task diagnostics', async function () {
    const inspection = await buildClusterInspection(
      'cluster-1',
      { sampleMs: 1 },
      {
        status: {
          id: 'cluster-1',
          state: 'running',
          isZombie: false,
          pid: 10,
          createdAt: Date.parse('2026-03-08T18:48:24.000Z'),
          messageCount: 30,
          agents: [
            {
              id: 'worker',
              role: 'implementation',
              model: 'sonnet',
              provider: 'claude',
              state: 'executing_task',
              iteration: 1,
              currentTask: true,
              currentTaskId: 'task-123',
              pid: 20,
            },
          ],
        },
        orchestrator: {},
        getTask: () => ({
          id: 'task-123',
          status: 'running',
          pid: 20,
          exitCode: null,
          error: null,
          cwd: '/tmp/work',
          sessionId: null,
          attachable: true,
          socketPath: '/tmp/task.sock',
          logFile: '/tmp/task.log',
          createdAt: '2026-03-08T18:52:01.000Z',
          updatedAt: '2026-03-08T18:52:01.000Z',
        }),
        existsSync: () => false,
      }
    );

    assert.strictEqual(inspection.type, 'cluster');
    assert.strictEqual(inspection.cluster.id, 'cluster-1');
    assert.strictEqual(inspection.agents.length, 1);
    assert.strictEqual(inspection.agents[0].task.id, 'task-123');
    assert(
      inspection.agents[0].task.warnings.some((warning) =>
        warning.includes('task log file missing')
      )
    );
  });

  it('should build task inspection payload', async function () {
    const inspection = await buildTaskInspection(
      'task-7',
      { sampleMs: 1 },
      {
        getTask: () => ({
          id: 'task-7',
          status: 'running',
          pid: null,
          exitCode: null,
          error: null,
          cwd: '/tmp/task',
          sessionId: 'sess-1',
          attachable: false,
          socketPath: null,
          logFile: null,
          createdAt: '2026-03-08T18:52:01.000Z',
          updatedAt: '2026-03-08T18:53:01.000Z',
        }),
        existsSync: () => false,
      }
    );

    assert.strictEqual(inspection.type, 'task');
    assert.strictEqual(inspection.task.id, 'task-7');
    assert.strictEqual(inspection.process, null);
  });
});
