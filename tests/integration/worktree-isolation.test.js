/**
 * Test: Worktree Isolation - Lightweight git-based isolation
 *
 * Tests the worktree isolation mode that provides:
 * - Git worktree creation at /tmp/zeroshot-worktrees/{clusterId}
 * - Separate branch (zeroshot/{clusterId}) without copying files
 * - Fast setup (<1s vs 30-60s for Docker)
 * - No Docker dependency
 *
 * REQUIRES: Git installed
 * NO Docker required (that's the point!)
 */

const assert = require('assert');
const path = require('path');
const fs = require('fs');
const os = require('os');
const { execSync } = require('child_process');

const IsolationManager = require('../../src/isolation-manager');

describe('IsolationManager - Worktree Mode', function () {
  this.timeout(30000);

  let manager;
  let testRepoDir;
  const testClusterId = 'test-worktree-' + Date.now();

  before(function () {
    // Create a temporary git repository for testing
    testRepoDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zs-worktree-test-repo-'));

    // Initialize git repo with initial commit
    execSync('git init', { cwd: testRepoDir, stdio: 'pipe' });
    execSync('git config user.email "test@test.com"', { cwd: testRepoDir, stdio: 'pipe' });
    execSync('git config user.name "Test User"', { cwd: testRepoDir, stdio: 'pipe' });

    // Create a test file and commit
    fs.writeFileSync(path.join(testRepoDir, 'test.txt'), 'initial content');
    execSync('git add -A', { cwd: testRepoDir, stdio: 'pipe' });
    execSync('git commit -m "Initial commit"', { cwd: testRepoDir, stdio: 'pipe' });

    manager = new IsolationManager();
  });

  afterEach(function () {
    // Clean up worktree after each test
    try {
      manager.cleanupWorktreeIsolation(testClusterId);
    } catch {
      // Ignore cleanup errors
    }
  });

  after(function () {
    // Clean up test repo
    if (testRepoDir && fs.existsSync(testRepoDir)) {
      fs.rmSync(testRepoDir, { recursive: true, force: true });
    }
  });

  describe('createWorktreeIsolation()', function () {
    it('should create worktree at expected path', function () {
      const info = manager.createWorktreeIsolation(testClusterId, testRepoDir);

      assert(info.path, 'Should return worktree path');
      assert(info.path.includes('/tmp/zeroshot-worktrees/'), 'Path should be in tmp');
      assert(info.path.includes(testClusterId), 'Path should include cluster ID');
      assert(fs.existsSync(info.path), 'Worktree directory should exist');
    });

    it('should create branch with zeroshot/ prefix', function () {
      const info = manager.createWorktreeIsolation(testClusterId, testRepoDir);

      assert(info.branch, 'Should return branch name');
      assert(info.branch.startsWith('zeroshot/'), `Branch should start with zeroshot/, got: ${info.branch}`);

      // Verify branch exists in main repo
      const branches = execSync('git branch --list', { cwd: testRepoDir, encoding: 'utf8' });
      assert(branches.includes(info.branch), `Branch ${info.branch} should exist`);
    });

    it('should return correct repoRoot', function () {
      const info = manager.createWorktreeIsolation(testClusterId, testRepoDir);

      assert.strictEqual(info.repoRoot, testRepoDir, 'repoRoot should match source directory');
    });

    it('should create working git repo in worktree', function () {
      const info = manager.createWorktreeIsolation(testClusterId, testRepoDir);

      // Verify it's a valid git worktree
      const gitDir = execSync('git rev-parse --git-dir', {
        cwd: info.path,
        encoding: 'utf8'
      }).trim();

      // Worktrees have .git file pointing to main repo's .git/worktrees/{name}
      assert(gitDir.includes('.git/worktrees/'), `Should be a worktree, got git-dir: ${gitDir}`);
    });

    it('should have same content as source repo', function () {
      const info = manager.createWorktreeIsolation(testClusterId, testRepoDir);

      // Verify test file exists in worktree
      const worktreeTestFile = path.join(info.path, 'test.txt');
      assert(fs.existsSync(worktreeTestFile), 'test.txt should exist in worktree');

      const content = fs.readFileSync(worktreeTestFile, 'utf8');
      assert.strictEqual(content, 'initial content', 'Content should match source');
    });

    it('should be on the new branch', function () {
      const info = manager.createWorktreeIsolation(testClusterId, testRepoDir);

      const currentBranch = execSync('git branch --show-current', {
        cwd: info.path,
        encoding: 'utf8'
      }).trim();

      assert.strictEqual(currentBranch, info.branch, 'Worktree should be on the new branch');
    });

    it('should allow commits in worktree', function () {
      const info = manager.createWorktreeIsolation(testClusterId, testRepoDir);

      // Create a new file in worktree
      fs.writeFileSync(path.join(info.path, 'new-file.txt'), 'worktree content');

      // Commit it
      execSync('git add new-file.txt', { cwd: info.path, stdio: 'pipe' });
      execSync('git commit -m "Add new file in worktree"', { cwd: info.path, stdio: 'pipe' });

      // Verify commit exists
      const log = execSync('git log --oneline', { cwd: info.path, encoding: 'utf8' });
      assert(log.includes('Add new file in worktree'), 'Commit should exist');
    });

    it('should isolate changes from main repo', function () {
      const info = manager.createWorktreeIsolation(testClusterId, testRepoDir);

      // Create file in worktree
      fs.writeFileSync(path.join(info.path, 'isolated-file.txt'), 'isolated content');
      execSync('git add isolated-file.txt', { cwd: info.path, stdio: 'pipe' });
      execSync('git commit -m "Isolated commit"', { cwd: info.path, stdio: 'pipe' });

      // Verify file does NOT exist in main repo working dir
      assert(
        !fs.existsSync(path.join(testRepoDir, 'isolated-file.txt')),
        'Isolated file should NOT appear in main repo'
      );
    });

    it('should throw for non-git directory', function () {
      const nonGitDir = fs.mkdtempSync(path.join(os.tmpdir(), 'non-git-'));

      try {
        manager.createWorktreeIsolation('test-fail', nonGitDir);
        assert.fail('Should throw for non-git directory');
      } catch (err) {
        assert(err.message.includes('git repository'), `Error should mention git: ${err.message}`);
      } finally {
        fs.rmSync(nonGitDir, { recursive: true, force: true });
      }
    });

    it('should clean up existing worktree before creating', function () {
      // Create worktree first time
      const info1 = manager.createWorktreeIsolation(testClusterId, testRepoDir);
      const path1 = info1.path;

      // Create a marker file
      fs.writeFileSync(path.join(path1, 'marker.txt'), 'first run');

      // Clean up manually (simulate orphaned state)
      manager.worktrees.delete(testClusterId);

      // Create worktree second time (should clean up old one)
      const info2 = manager.createWorktreeIsolation(testClusterId, testRepoDir);

      // Marker should NOT exist (fresh worktree)
      assert(
        !fs.existsSync(path.join(info2.path, 'marker.txt')),
        'Old marker should be removed (fresh worktree)'
      );
    });
  });

  describe('cleanupWorktreeIsolation()', function () {
    it('should remove worktree directory', function () {
      const info = manager.createWorktreeIsolation(testClusterId, testRepoDir);
      const worktreePath = info.path;

      assert(fs.existsSync(worktreePath), 'Worktree should exist before cleanup');

      manager.cleanupWorktreeIsolation(testClusterId);

      assert(!fs.existsSync(worktreePath), 'Worktree directory should be removed');
    });

    it('should remove worktree from git tracking', function () {
      const _info = manager.createWorktreeIsolation(testClusterId, testRepoDir);

      // Verify worktree is tracked
      const beforeList = execSync('git worktree list', { cwd: testRepoDir, encoding: 'utf8' });
      assert(beforeList.includes(testClusterId), 'Worktree should be tracked before cleanup');

      manager.cleanupWorktreeIsolation(testClusterId);

      // Verify worktree is no longer tracked
      const afterList = execSync('git worktree list', { cwd: testRepoDir, encoding: 'utf8' });
      assert(!afterList.includes(testClusterId), 'Worktree should not be tracked after cleanup');
    });

    it('should preserve branch by default', function () {
      const info = manager.createWorktreeIsolation(testClusterId, testRepoDir);
      const branchName = info.branch;

      manager.cleanupWorktreeIsolation(testClusterId);

      // Branch should still exist (for PR creation)
      const branches = execSync('git branch --list', { cwd: testRepoDir, encoding: 'utf8' });
      assert(branches.includes(branchName), 'Branch should be preserved after cleanup');
    });

    it('should be idempotent (no error on double cleanup)', function () {
      const _info = manager.createWorktreeIsolation(testClusterId, testRepoDir);

      manager.cleanupWorktreeIsolation(testClusterId);

      // Second cleanup should not throw
      manager.cleanupWorktreeIsolation(testClusterId);
    });

    it('should not error for non-existent worktree', function () {
      // Should not throw
      manager.cleanupWorktreeIsolation('non-existent-cluster-xyz');
    });
  });

  describe('getWorktreeInfo()', function () {
    it('should return worktree info after creation', function () {
      manager.createWorktreeIsolation(testClusterId, testRepoDir);

      const info = manager.getWorktreeInfo(testClusterId);

      assert(info, 'Should return info');
      assert(info.path, 'Should have path');
      assert(info.branch, 'Should have branch');
      assert(info.repoRoot, 'Should have repoRoot');
    });

    it('should return undefined for non-existent cluster', function () {
      const info = manager.getWorktreeInfo('non-existent-xyz');

      assert.strictEqual(info, undefined, 'Should return undefined');
    });

    it('should return undefined after cleanup', function () {
      manager.createWorktreeIsolation(testClusterId, testRepoDir);
      manager.cleanupWorktreeIsolation(testClusterId);

      const info = manager.getWorktreeInfo(testClusterId);

      assert.strictEqual(info, undefined, 'Should return undefined after cleanup');
    });
  });

  describe('Performance', function () {
    it('should create worktree in under 1 second', function () {
      const startTime = Date.now();

      manager.createWorktreeIsolation(testClusterId, testRepoDir);

      const elapsed = Date.now() - startTime;

      assert(elapsed < 1000, `Worktree creation should be <1s, took ${elapsed}ms`);
    });
  });
});
