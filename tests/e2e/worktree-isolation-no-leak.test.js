/**
 * Tier 1 e2e: --worktree mode leaves the main checkout untouched and
 * registers a real git worktree/branch for the cluster, with no leaked
 * process for the completed cluster.
 */

const assert = require('node:assert');
const fs = require('fs');
const os = require('os');
const path = require('path');

const {
  setupE2ERepo,
  cleanupE2ERepo,
  runZeroshot,
  worktreePath,
  gitStatusPorcelain,
  gitWorktreeList,
  scenarioPath,
} = require('./helpers/e2e-harness');

const CONFIG_PATH = path.join(__dirname, 'fixtures', 'single-worker-config.json');

describe('e2e: worktree isolation, no leak', function () {
  this.timeout(60000);

  let env;

  beforeEach(() => {
    env = setupE2ERepo();
  });

  afterEach(() => {
    cleanupE2ERepo(env);
  });

  it('leaves the main checkout clean and registers a real worktree/branch', function () {
    const issueDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-e2e-issue-'));
    const issuePath = path.join(issueDir, 'feature.md');
    fs.writeFileSync(issuePath, '# Add feature\n\nDo X.\n');

    const clusterId = 'e2e-worktree-isolation';
    const result = runZeroshot(env, ['run', issuePath, '--worktree', '--config', CONFIG_PATH], {
      ZEROSHOT_CLUSTER_ID: clusterId,
      FAKE_AGENT_SCENARIO: scenarioPath('single-worker-success'),
    });

    assert.strictEqual(
      result.status,
      0,
      `zeroshot run exited ${result.status}\nSTDOUT:\n${result.stdout}\nSTDERR:\n${result.stderr}`
    );

    assert.strictEqual(
      gitStatusPorcelain(env.repoDir),
      '',
      'main checkout should have no uncommitted residue'
    );

    const worktreeList = gitWorktreeList(env.repoDir);
    assert.ok(
      worktreeList.includes(`zeroshot/${clusterId}`),
      `expected git worktree list to include zeroshot/${clusterId}, got:\n${worktreeList}`
    );
    assert.ok(fs.existsSync(worktreePath(env, clusterId)), 'worktree directory should exist');

    const psOutput = require('child_process').execSync('ps -A -o command=').toString();
    const leakedLines = psOutput
      .split('\n')
      .filter((line) => line.includes(clusterId) && !line.includes('grep'));
    assert.strictEqual(
      leakedLines.length,
      0,
      `expected no leaked processes referencing ${clusterId}, found:\n${leakedLines.join('\n')}`
    );

    fs.rmSync(issueDir, { recursive: true, force: true });
  });
});
