/**
 * Tier 1 e2e: worker -> validator flow with distinct fake-agent scenarios per
 * agent. Both agents share one process-wide ZEROSHOT_CLAUDE_COMMAND, so
 * per-agent scenario dispatch relies on the FAKE_AGENT_ID=<id> marker
 * embedded in each agent's `prompt` field (see tests/fixtures/fake-agent).
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
  readLedgerMessages,
  scenarioPath,
} = require('./helpers/e2e-harness');

const CONFIG_PATH = path.join(__dirname, 'fixtures', 'worker-validator-config.json');

describe('e2e: worker -> validator', function () {
  this.timeout(60000);

  let env;

  beforeEach(() => {
    env = setupE2ERepo();
  });

  afterEach(() => {
    cleanupE2ERepo(env);
  });

  it('runs both agents with distinct scenarios and reaches validation approval', function () {
    const issueDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-e2e-issue-'));
    const issuePath = path.join(issueDir, 'feature.md');
    fs.writeFileSync(issuePath, '# Add feature\n\nDo X.\n');

    const clusterId = 'e2e-worker-validator';
    const result = runZeroshot(env, ['run', issuePath, '--worktree', '--config', CONFIG_PATH], {
      ZEROSHOT_CLUSTER_ID: clusterId,
      FAKE_AGENT_SCENARIO_WORKER: scenarioPath('worker-success'),
      FAKE_AGENT_SCENARIO_VALIDATOR: scenarioPath('validator-approve'),
    });

    assert.strictEqual(
      result.status,
      0,
      `zeroshot run exited ${result.status}\nSTDOUT:\n${result.stdout}\nSTDERR:\n${result.stderr}`
    );

    const worktreeDir = worktreePath(env, clusterId);
    assert.ok(
      fs.existsSync(path.join(worktreeDir, 'implementation.txt')),
      'worker should have written implementation.txt'
    );
    assert.ok(
      fs.existsSync(path.join(worktreeDir, 'validation-report.txt')),
      'validator should have written validation-report.txt'
    );

    const implementationReady = readLedgerMessages(env, clusterId, 'IMPLEMENTATION_READY');
    assert.strictEqual(implementationReady.length, 1);
    assert.strictEqual(implementationReady[0].sender, 'worker');

    const validationResults = readLedgerMessages(env, clusterId, 'VALIDATION_RESULT');
    assert.strictEqual(validationResults.length, 1);
    assert.strictEqual(validationResults[0].sender, 'validator');
    assert.strictEqual(validationResults[0].content.data.approved, true);

    fs.rmSync(issueDir, { recursive: true, force: true });
  });
});
