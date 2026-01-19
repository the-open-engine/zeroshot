/**
 * Test: Worktree Isolation - Lightweight git-based isolation
 *
 * Tests the worktree isolation mode that provides:
 * - Git worktree creation at {os.tmpdir()}/zeroshot-worktrees/{clusterId}
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

let manager;
let testRepoDir;
const testClusterId = 'test-worktree-' + Date.now();

function registerRepositoryHooks() {
  before(function () {
    testRepoDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zs-worktree-test-repo-'));

    execSync('git init', { cwd: testRepoDir, stdio: 'pipe' });
    execSync('git config user.email "test@test.com"', { cwd: testRepoDir, stdio: 'pipe' });
    execSync('git config user.name "Test User"', { cwd: testRepoDir, stdio: 'pipe' });

    fs.writeFileSync(path.join(testRepoDir, 'test.txt'), 'initial content');
    execSync('git add -A', { cwd: testRepoDir, stdio: 'pipe' });
    execSync('git commit -m "Initial commit"', { cwd: testRepoDir, stdio: 'pipe' });

    manager = new IsolationManager();
  });

  afterEach(function () {
    try {
      manager.cleanupWorktreeIsolation(testClusterId);
    } catch {
      // Ignore cleanup errors
    }
  });

  after(function () {
    if (testRepoDir && fs.existsSync(testRepoDir)) {
      fs.rmSync(testRepoDir, { recursive: true, force: true });
    }
  });
}

function registerCreateWorktreeIsolationTests() {
  describe('createWorktreeIsolation()', function () {
    registerWorktreePathTest();
    registerWorktreeBranchTest();
    registerWorktreeRepoRootTest();
    registerWorktreeGitRepoTest();
    registerWorktreeContentTest();
    registerWorktreeBranchCheckoutTest();
    registerWorktreeCommitTest();
    registerWorktreeIsolationTest();
    registerWorktreeNonGitTest();
    registerWorktreeCleanupBeforeCreateTest();
  });
}

function registerWorktreePathTest() {
  it('should create worktree at expected path', function () {
    const info = manager.createWorktreeIsolation(testClusterId, testRepoDir);

    assert(info.path, 'Should return worktree path');
    const expectedRoot = fs.realpathSync(path.join(os.tmpdir(), 'zeroshot-worktrees'));
    const worktreePath = fs.realpathSync(info.path);
    assert(
      worktreePath.startsWith(expectedRoot + path.sep),
      `Path should be in ${expectedRoot}${path.sep}`
    );
    assert(info.path.includes(testClusterId), 'Path should include cluster ID');
    assert(fs.existsSync(info.path), 'Worktree directory should exist');
  });
}

function registerWorktreeBranchTest() {
  it('should create branch with zeroshot/ prefix', function () {
    const info = manager.createWorktreeIsolation(testClusterId, testRepoDir);

    assert(info.branch, 'Should return branch name');
    assert(
      info.branch.startsWith('zeroshot/'),
      `Branch should start with zeroshot/, got: ${info.branch}`
    );

    const branches = execSync('git branch --list', { cwd: testRepoDir, encoding: 'utf8' });
    assert(branches.includes(info.branch), `Branch ${info.branch} should exist`);
  });
}

function registerWorktreeRepoRootTest() {
  it('should return correct repoRoot', function () {
    const info = manager.createWorktreeIsolation(testClusterId, testRepoDir);

    assert.strictEqual(
      fs.realpathSync(info.repoRoot),
      fs.realpathSync(testRepoDir),
      'repoRoot should match source directory'
    );
  });
}

function registerWorktreeGitRepoTest() {
  it('should create working git repo in worktree', function () {
    const info = manager.createWorktreeIsolation(testClusterId, testRepoDir);

    const gitDir = execSync('git rev-parse --git-dir', {
      cwd: info.path,
      encoding: 'utf8',
    }).trim();

    assert(gitDir.includes('.git/worktrees/'), `Should be a worktree, got git-dir: ${gitDir}`);
  });
}

function registerWorktreeContentTest() {
  it('should have same content as source repo', function () {
    const info = manager.createWorktreeIsolation(testClusterId, testRepoDir);

    const worktreeTestFile = path.join(info.path, 'test.txt');
    assert(fs.existsSync(worktreeTestFile), 'test.txt should exist in worktree');

    const content = fs.readFileSync(worktreeTestFile, 'utf8');
    assert.strictEqual(content, 'initial content', 'Content should match source');
  });
}

function registerWorktreeBranchCheckoutTest() {
  it('should be on the new branch', function () {
    const info = manager.createWorktreeIsolation(testClusterId, testRepoDir);

    const currentBranch = execSync('git branch --show-current', {
      cwd: info.path,
      encoding: 'utf8',
    }).trim();

    assert.strictEqual(currentBranch, info.branch, 'Worktree should be on the new branch');
  });
}

function registerWorktreeCommitTest() {
  it('should allow commits in worktree', function () {
    const info = manager.createWorktreeIsolation(testClusterId, testRepoDir);

    fs.writeFileSync(path.join(info.path, 'new-file.txt'), 'worktree content');

    execSync('git add new-file.txt', { cwd: info.path, stdio: 'pipe' });
    execSync('git commit -m "Add new file in worktree"', { cwd: info.path, stdio: 'pipe' });

    const log = execSync('git log --oneline', { cwd: info.path, encoding: 'utf8' });
    assert(log.includes('Add new file in worktree'), 'Commit should exist');
  });
}

function registerWorktreeIsolationTest() {
  it('should isolate changes from main repo', function () {
    const info = manager.createWorktreeIsolation(testClusterId, testRepoDir);

    fs.writeFileSync(path.join(info.path, 'isolated-file.txt'), 'isolated content');
    execSync('git add isolated-file.txt', { cwd: info.path, stdio: 'pipe' });
    execSync('git commit -m "Isolated commit"', { cwd: info.path, stdio: 'pipe' });

    assert(
      !fs.existsSync(path.join(testRepoDir, 'isolated-file.txt')),
      'Isolated file should NOT appear in main repo'
    );
  });
}

function registerWorktreeNonGitTest() {
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
}

function registerWorktreeCleanupBeforeCreateTest() {
  it('should clean up existing worktree before creating', function () {
    const info1 = manager.createWorktreeIsolation(testClusterId, testRepoDir);
    const path1 = info1.path;

    fs.writeFileSync(path.join(path1, 'marker.txt'), 'first run');

    manager.worktrees.delete(testClusterId);

    const info2 = manager.createWorktreeIsolation(testClusterId, testRepoDir);

    assert(
      !fs.existsSync(path.join(info2.path, 'marker.txt')),
      'Old marker should be removed (fresh worktree)'
    );
  });
}

function registerCleanupWorktreeIsolationTests() {
  describe('cleanupWorktreeIsolation()', function () {
    registerCleanupWorktreeDirectoryTest();
    registerCleanupWorktreeTrackingTest();
    registerCleanupWorktreeBranchTest();
    registerCleanupWorktreeIdempotentTest();
    registerCleanupWorktreeMissingTest();
  });
}

function registerCleanupWorktreeDirectoryTest() {
  it('should remove worktree directory', function () {
    const info = manager.createWorktreeIsolation(testClusterId, testRepoDir);
    const worktreePath = info.path;

    assert(fs.existsSync(worktreePath), 'Worktree should exist before cleanup');

    manager.cleanupWorktreeIsolation(testClusterId);

    assert(!fs.existsSync(worktreePath), 'Worktree directory should be removed');
  });
}

function registerCleanupWorktreeTrackingTest() {
  it('should remove worktree from git tracking', function () {
    manager.createWorktreeIsolation(testClusterId, testRepoDir);

    const beforeList = execSync('git worktree list', { cwd: testRepoDir, encoding: 'utf8' });
    assert(beforeList.includes(testClusterId), 'Worktree should be tracked before cleanup');

    manager.cleanupWorktreeIsolation(testClusterId);

    const afterList = execSync('git worktree list', { cwd: testRepoDir, encoding: 'utf8' });
    assert(!afterList.includes(testClusterId), 'Worktree should not be tracked after cleanup');
  });
}

function registerCleanupWorktreeBranchTest() {
  it('should preserve branch by default', function () {
    const info = manager.createWorktreeIsolation(testClusterId, testRepoDir);
    const branchName = info.branch;

    manager.cleanupWorktreeIsolation(testClusterId);

    const branches = execSync('git branch --list', { cwd: testRepoDir, encoding: 'utf8' });
    assert(branches.includes(branchName), 'Branch should be preserved after cleanup');
  });
}

function registerCleanupWorktreeIdempotentTest() {
  it('should be idempotent (no error on double cleanup)', function () {
    manager.createWorktreeIsolation(testClusterId, testRepoDir);

    manager.cleanupWorktreeIsolation(testClusterId);

    manager.cleanupWorktreeIsolation(testClusterId);
  });
}

function registerCleanupWorktreeMissingTest() {
  it('should not error for non-existent worktree', function () {
    manager.cleanupWorktreeIsolation('non-existent-cluster-xyz');
  });
}

function registerGetWorktreeInfoTests() {
  describe('getWorktreeInfo()', function () {
    registerGetWorktreeInfoAfterCreateTest();
    registerGetWorktreeInfoMissingTest();
    registerGetWorktreeInfoAfterCleanupTest();
  });
}

function registerGetWorktreeInfoAfterCreateTest() {
  it('should return worktree info after creation', function () {
    manager.createWorktreeIsolation(testClusterId, testRepoDir);

    const info = manager.getWorktreeInfo(testClusterId);

    assert(info, 'Should return info');
    assert(info.path, 'Should have path');
    assert(info.branch, 'Should have branch');
    assert(info.repoRoot, 'Should have repoRoot');
  });
}

function registerGetWorktreeInfoMissingTest() {
  it('should return undefined for non-existent cluster', function () {
    const info = manager.getWorktreeInfo('non-existent-xyz');

    assert.strictEqual(info, undefined, 'Should return undefined');
  });
}

function registerGetWorktreeInfoAfterCleanupTest() {
  it('should return undefined after cleanup', function () {
    manager.createWorktreeIsolation(testClusterId, testRepoDir);
    manager.cleanupWorktreeIsolation(testClusterId);

    const info = manager.getWorktreeInfo(testClusterId);

    assert.strictEqual(info, undefined, 'Should return undefined after cleanup');
  });
}

function registerWorktreePerformanceTests() {
  describe('Performance', function () {
    it('should create worktree in under 1 second', function () {
      const startTime = Date.now();

      manager.createWorktreeIsolation(testClusterId, testRepoDir);

      const elapsed = Date.now() - startTime;

      assert(elapsed < 1000, `Worktree creation should be <1s, took ${elapsed}ms`);
    });
  });
}

describe('IsolationManager - Worktree Mode', function () {
  this.timeout(30000);

  registerRepositoryHooks();
  registerCreateWorktreeIsolationTests();
  registerCleanupWorktreeIsolationTests();
  registerGetWorktreeInfoTests();
  registerWorktreePerformanceTests();
});
