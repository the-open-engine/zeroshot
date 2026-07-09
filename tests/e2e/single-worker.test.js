/**
 * Tier 1 e2e: full pipeline through the real `zeroshot` binary with the
 * fake provider CLI standing in for the model. Proves: CLI parsing -> config
 * load -> orchestrator start -> real subprocess spawn -> stream-json parsing
 * -> hook-driven completion -> file write happens in the worktree, not the
 * main checkout.
 */

const assert = require('node:assert');
const fs = require('fs');
const os = require('os');
const path = require('path');

const {
  setupE2ERepo,
  cleanupE2ERepo,
  runZeroshot,
  readCluster,
  worktreePath,
  waitForClusterState,
  scenarioPath,
  gitStatusPorcelain,
} = require('./helpers/e2e-harness');

const CONFIG_PATH = path.join(__dirname, 'fixtures', 'single-worker-config.json');

describe('e2e: single worker', function () {
  this.timeout(60000);

  let env;

  beforeEach(() => {
    env = setupE2ERepo();
  });

  afterEach(() => {
    cleanupE2ERepo(env);
  });

  it('runs the full pipeline and writes the file into the worktree, not the main checkout', async function () {
    // The issue file lives outside the repo so the main checkout's git status
    // stays meaningful as a "did anything leak in" signal.
    const issueDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-e2e-issue-'));
    const issuePath = path.join(issueDir, 'feature.md');
    fs.writeFileSync(issuePath, '# Add feature\n\nDo X.\n');

    const clusterId = 'e2e-single-worker';
    const result = runZeroshot(env, ['run', issuePath, '--worktree', '--config', CONFIG_PATH], {
      ZEROSHOT_CLUSTER_ID: clusterId,
      FAKE_AGENT_SCENARIO: scenarioPath('single-worker-success'),
    });

    assert.strictEqual(
      result.status,
      0,
      `zeroshot run exited ${result.status}\nSTDOUT:\n${result.stdout}\nSTDERR:\n${result.stderr}`
    );

    const cluster = await waitForClusterState(env, clusterId, ['stopped', 'killed']);
    const worktreeDir = worktreePath(env, clusterId);
    assert.ok(fs.existsSync(worktreeDir), 'worktree directory should exist');

    const writtenFile = path.join(worktreeDir, 'output.txt');
    assert.ok(fs.existsSync(writtenFile), `expected ${writtenFile} to exist`);
    assert.strictEqual(fs.readFileSync(writtenFile, 'utf8'), 'feature implemented\n');

    assert.ok(
      !fs.existsSync(path.join(env.repoDir, 'output.txt')),
      'output.txt must not leak into the main checkout'
    );
    assert.strictEqual(gitStatusPorcelain(env.repoDir), '', 'main checkout should remain clean');

    const clusterAfter = readCluster(env, clusterId);
    assert.strictEqual(clusterAfter.state, cluster.state);

    fs.rmSync(issueDir, { recursive: true, force: true });
  });
});
