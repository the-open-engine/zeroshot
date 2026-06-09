const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const childProcess = require('child_process');

describe('Worktree detached process cleanup', function () {
  this.timeout(10000);

  let tmpDir;

  beforeEach(function () {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-worktree-procs-'));
  });

  afterEach(function () {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it('kills detached daemon families before removing a worktree', function () {
    const origSpawnSync = childProcess.spawnSync;
    const origProcessKill = process.kill;
    const calls = [];

    try {
      childProcess.spawnSync = function (command, args, opts) {
        calls.push({ type: 'spawn', command, args, cwd: opts?.cwd });

        if (command === 'ps' && args.join(' ') === 'axww -o pid=,command=') {
          const fakeWorktreePath = path.join(tmpDir, 'worktree');
          return {
            status: 0,
            stdout: [
              `111 node /tmp/daemon-server.js --rootDir ${fakeWorktreePath}`,
              `112 node /tmp/rust-daemon-server.js --rootDir ${fakeWorktreePath}`,
              `113 node /tmp/tsserver-daemon-server.js --rootDir ${fakeWorktreePath}`,
              `114 node /tmp/eslint-daemon-server.js --rootDir ${fakeWorktreePath}`,
              `115 rust-analyzer ${path.join(fakeWorktreePath, 'src', 'main.ts')}`,
              `222 node /tmp/other-daemon.cjs --repo ${path.join(tmpDir, 'elsewhere')}`,
            ].join('\n'),
            stderr: '',
          };
        }

        if (command === 'git' && args[0] === 'worktree') {
          return { status: 1, stdout: '', stderr: `not a git repo: ${opts?.cwd || tmpDir}` };
        }

        return { status: 0, stdout: '', stderr: '' };
      };

      process.kill = function (pid, signal) {
        calls.push({ type: 'kill', pid, signal });
      };

      delete require.cache[require.resolve('../src/isolation-manager')];
      const IsolationManager = require('../src/isolation-manager');
      const manager = new IsolationManager();

      const fakeWorktreePath = path.join(tmpDir, 'worktree');
      fs.mkdirSync(fakeWorktreePath, { recursive: true });

      manager.worktrees.set('test-cluster', {
        path: fakeWorktreePath,
        branch: 'zeroshot/test-cluster',
        repoRoot: tmpDir,
      });

      manager.cleanupWorktreeIsolation('test-cluster');

      const killCalls = calls.filter((entry) => entry.type === 'kill');
      assert.deepStrictEqual(
        killCalls,
        [
          { type: 'kill', pid: 111, signal: 'SIGTERM' },
          { type: 'kill', pid: 112, signal: 'SIGTERM' },
          { type: 'kill', pid: 113, signal: 'SIGTERM' },
          { type: 'kill', pid: 114, signal: 'SIGTERM' },
          { type: 'kill', pid: 115, signal: 'SIGTERM' },
        ],
        'cleanup should SIGTERM only daemon-family processes whose argv references the worktree path'
      );

      const killIdx = calls.findIndex((entry) => entry.type === 'kill');
      const gitIdx = calls.findIndex(
        (entry) =>
          entry.type === 'spawn' &&
          entry.command === 'git' &&
          entry.args.join(' ') === `worktree remove --force ${fakeWorktreePath}`
      );
      assert.ok(killIdx !== -1, 'expected a kill call');
      assert.ok(gitIdx !== -1, 'expected git worktree removal to be attempted');
      assert.ok(killIdx < gitIdx, 'worktree-scoped processes must be killed before removal');
    } finally {
      process.kill = origProcessKill;
      childProcess.spawnSync = origSpawnSync;
      delete require.cache[require.resolve('../src/isolation-manager')];
    }
  });

  it('reaps worktree-scoped processes on stop when the worktree is preserved', async function () {
    const Orchestrator = require('../src/orchestrator');
    const orchestrator = new Orchestrator({ quiet: true, storageDir: tmpDir });
    const fakeWorktreePath = path.join(tmpDir, 'preserved-worktree');
    fs.mkdirSync(fakeWorktreePath, { recursive: true });

    const calls = [];
    orchestrator._saveClusters = async () => {};
    orchestrator._signalRemoteCluster = () => ({ handled: false });

    orchestrator.clusters.set('cluster-preserve', {
      id: 'cluster-preserve',
      pid: null,
      state: 'running',
      agents: [],
      snapshotter: null,
      isolation: null,
      validatorIsolation: null,
      initCompletePromise: null,
      autoPr: true,
      worktree: {
        path: fakeWorktreePath,
        branch: 'zeroshot/cluster-preserve',
        manager: {
          cleanupWorktreeProcesses(worktreePath) {
            calls.push(worktreePath);
          },
        },
      },
    });

    await orchestrator.stop('cluster-preserve');

    assert.deepStrictEqual(
      calls,
      [fakeWorktreePath],
      'stop() should reap worktree-scoped daemons before preserving the worktree for resume'
    );
  });
});
