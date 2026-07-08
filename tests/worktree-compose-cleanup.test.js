/**
 * Worktree Docker Compose Cleanup Test Suite
 *
 * Regression test for bug where `docker compose down` was never called when
 * cleaning up zeroshot worktrees. Agents could run `docker compose up` inside
 * worktrees, and those containers would keep running after session end,
 * hogging host ports (5433, 6379, 3001, etc.) and blocking the main project.
 *
 * Root cause: removeWorktree() only did `git worktree remove` + `fs.rmSync`,
 * never tore down Docker Compose services. The orchestrator stop() path
 * (Ctrl+C / SIGINT) preserved worktrees entirely, including running containers.
 *
 * Fix:
 * - isolation-manager.js: removeWorktree() now calls `docker compose down` first
 * - orchestrator.js: stop() calls _teardownWorktreeCompose() even when preserving worktree
 */

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const os = require('os');
const childProcess = require('child_process');

function isComposeDownCall(call) {
  return (
    call.command === 'docker' &&
    Array.isArray(call.args) &&
    call.args[0] === 'compose' &&
    call.args[1] === 'down'
  );
}

describe('Worktree Docker Compose Cleanup', function () {
  this.timeout(10000);

  let tmpDir;

  beforeEach(function () {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-compose-cleanup-'));
  });

  afterEach(function () {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  describe('IsolationManager.removeWorktree', function () {
    it('should call docker compose down before removing worktree with docker-compose.yml', function () {
      const origSpawnSync = childProcess.spawnSync;
      const calls = [];

      childProcess.spawnSync = function (command, args, opts) {
        calls.push({ command, args, cwd: opts?.cwd });
        if (command === 'git' && args[0] === 'worktree') {
          return { status: 1, stdout: '', stderr: 'not a git repo' };
        }
        return { status: 0, stdout: '', stderr: '' };
      };

      try {
        // Re-require because IsolationManager binds child_process helpers at import time.
        delete require.cache[require.resolve('../src/isolation-manager')];
        const IsolationManager = require('../src/isolation-manager');
        const manager = new IsolationManager();

        // Create a fake worktree directory with docker-compose.yml
        const fakeWorktreePath = path.join(tmpDir, 'test-worktree');
        fs.mkdirSync(fakeWorktreePath, { recursive: true });
        fs.writeFileSync(path.join(fakeWorktreePath, 'docker-compose.yml'), 'version: "3"');

        // Register worktree in manager's internal map
        manager.worktrees.set('test-cluster', {
          path: fakeWorktreePath,
          branch: 'zeroshot/test-cluster',
          repoRoot: tmpDir,
        });

        manager.cleanupWorktreeIsolation('test-cluster');

        // Verify docker compose down was called
        const composeDownCall = calls.find(isComposeDownCall);
        assert.ok(composeDownCall, 'docker compose down should be called during cleanup');
        assert.strictEqual(
          composeDownCall.cwd,
          fakeWorktreePath,
          'docker compose down should run in the worktree directory'
        );
        assert.ok(
          composeDownCall.args.includes('--remove-orphans'),
          'docker compose down should use --remove-orphans'
        );
        assert.ok(
          composeDownCall.args.includes('--volumes'),
          'docker compose down should use --volumes to free disk'
        );

        // Verify ordering: compose down appears before git worktree remove
        const composeIdx = calls.findIndex(isComposeDownCall);
        const gitIdx = calls.findIndex(
          (c) =>
            c.command === 'git' &&
            c.args.join(' ') === `worktree remove --force ${fakeWorktreePath}`
        );
        assert.ok(
          composeIdx < gitIdx,
          `docker compose down (idx ${composeIdx}) should run before git worktree remove (idx ${gitIdx})`
        );
      } finally {
        childProcess.spawnSync = origSpawnSync;
        delete require.cache[require.resolve('../src/isolation-manager')];
      }
    });

    it('should skip docker compose down when no docker-compose.yml exists', function () {
      const origSpawnSync = childProcess.spawnSync;
      const calls = [];

      childProcess.spawnSync = function (command, args, opts) {
        calls.push({ command, args, cwd: opts?.cwd });
        if (command === 'git' && args[0] === 'worktree') {
          return { status: 1, stdout: '', stderr: 'not a git repo' };
        }
        return { status: 0, stdout: '', stderr: '' };
      };

      try {
        delete require.cache[require.resolve('../src/isolation-manager')];
        const IsolationManager = require('../src/isolation-manager');
        const manager = new IsolationManager();

        // Create a fake worktree directory WITHOUT docker-compose.yml
        const fakeWorktreePath = path.join(tmpDir, 'test-worktree-no-compose');
        fs.mkdirSync(fakeWorktreePath, { recursive: true });

        manager.worktrees.set('test-no-compose', {
          path: fakeWorktreePath,
          branch: 'zeroshot/test-no-compose',
          repoRoot: tmpDir,
        });

        manager.cleanupWorktreeIsolation('test-no-compose');

        const composeCall = calls.find(
          (c) => c.command === 'docker' && Array.isArray(c.args) && c.args[0] === 'compose'
        );
        assert.strictEqual(
          composeCall,
          undefined,
          'docker compose should NOT be called when no docker-compose.yml exists'
        );
      } finally {
        childProcess.spawnSync = origSpawnSync;
        delete require.cache[require.resolve('../src/isolation-manager')];
      }
    });

    it('should not fail when docker compose down throws', function () {
      const origSpawnSync = childProcess.spawnSync;

      childProcess.spawnSync = function (command, args) {
        if (command === 'docker' && args[0] === 'compose') {
          return { status: 1, stdout: '', stderr: 'Docker daemon not running' };
        }
        if (command === 'git') {
          return { status: 1, stdout: '', stderr: 'not a git repo' };
        }
        return { status: 0, stdout: '', stderr: '' };
      };

      try {
        delete require.cache[require.resolve('../src/isolation-manager')];
        const IsolationManager = require('../src/isolation-manager');
        const manager = new IsolationManager();

        const fakeWorktreePath = path.join(tmpDir, 'test-worktree-compose-fail');
        fs.mkdirSync(fakeWorktreePath, { recursive: true });
        fs.writeFileSync(path.join(fakeWorktreePath, 'docker-compose.yml'), 'version: "3"');

        manager.worktrees.set('test-compose-fail', {
          path: fakeWorktreePath,
          branch: 'zeroshot/test-compose-fail',
          repoRoot: tmpDir,
        });

        // Should not throw — compose down failure is best-effort
        assert.doesNotThrow(() => {
          manager.cleanupWorktreeIsolation('test-compose-fail');
        });
      } finally {
        childProcess.spawnSync = origSpawnSync;
        delete require.cache[require.resolve('../src/isolation-manager')];
      }
    });
  });

  describe('Orchestrator._teardownWorktreeCompose', function () {
    let origSpawnSync;

    beforeEach(function () {
      origSpawnSync = childProcess.spawnSync;
    });

    afterEach(function () {
      childProcess.spawnSync = origSpawnSync;
    });

    it('should tear down compose services during stop (Ctrl+C path)', function () {
      const Orchestrator = require('../src/orchestrator');
      const orchestrator = new Orchestrator({ dataDir: tmpDir });

      const fakeWorktreePath = path.join(tmpDir, 'stop-test-worktree');
      fs.mkdirSync(fakeWorktreePath, { recursive: true });
      fs.writeFileSync(path.join(fakeWorktreePath, 'docker-compose.yml'), 'version: "3"');

      const calls = [];
      childProcess.spawnSync = function (command, args, opts) {
        calls.push({ command, args, cwd: opts?.cwd });
        return { status: 0, stdout: '', stderr: '' };
      };

      orchestrator._teardownWorktreeCompose(fakeWorktreePath);

      const composeCall = calls.find(isComposeDownCall);
      assert.ok(composeCall, '_teardownWorktreeCompose should call docker compose down');
      assert.strictEqual(composeCall.cwd, fakeWorktreePath);
      assert.ok(composeCall.args.includes('--remove-orphans'));
      assert.ok(composeCall.args.includes('--volumes'));
    });

    it('should skip when no docker-compose.yml exists', function () {
      const Orchestrator = require('../src/orchestrator');
      const orchestrator = new Orchestrator({ dataDir: tmpDir });

      const fakeWorktreePath = path.join(tmpDir, 'no-compose-worktree');
      fs.mkdirSync(fakeWorktreePath, { recursive: true });

      const calls = [];
      childProcess.spawnSync = function (command, args, opts) {
        calls.push({ command, args, cwd: opts?.cwd });
        return { status: 0, stdout: '', stderr: '' };
      };

      orchestrator._teardownWorktreeCompose(fakeWorktreePath);
      assert.strictEqual(
        calls.length,
        0,
        'No commands should be called without docker-compose.yml'
      );
    });

    it('should not throw when docker compose down fails', function () {
      const Orchestrator = require('../src/orchestrator');
      const orchestrator = new Orchestrator({ dataDir: tmpDir });

      const fakeWorktreePath = path.join(tmpDir, 'fail-compose-worktree');
      fs.mkdirSync(fakeWorktreePath, { recursive: true });
      fs.writeFileSync(path.join(fakeWorktreePath, 'docker-compose.yml'), 'version: "3"');

      childProcess.spawnSync = function (command, args) {
        if (command === 'docker' && args[0] === 'compose') {
          return { status: 1, stdout: '', stderr: 'no such service' };
        }
        return { status: 0, stdout: '', stderr: '' };
      };

      assert.doesNotThrow(() => {
        orchestrator._teardownWorktreeCompose(fakeWorktreePath);
      });
    });
  });
});
