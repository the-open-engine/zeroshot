/**
 * Tier 1 e2e for the Copilot provider engine: the full pipeline through the real
 * `zeroshot` binary with a fake `copilot` CLI (on PATH) standing in for the model.
 *
 * Unlike the claude-based e2e (which shims the binary via ZEROSHOT_CLAUDE_COMMAND), a
 * generic registry provider is resolved from PATH by binary name. The harness prepends an
 * isolated bin dir to PATH, so installing an executable named `copilot` there exercises for
 * real: CLI parsing -> `--provider copilot` override -> registry resolution -> preflight
 * availability probe -> subprocess spawn -> Copilot `--output-format json` JSONL parsing (the
 * new copilot adapter/parser) -> hook-driven completion -> worktree-isolated file write.
 *
 * Fully offline: no Copilot API calls, no credentials, no network.
 */

const assert = require('node:assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawnSync } = require('node:child_process');

const {
  setupE2ERepo,
  cleanupE2ERepo,
  runZeroshot,
  worktreePath,
  waitForClusterState,
  readLedgerMessages,
  gitStatusPorcelain,
} = require('./helpers/e2e-harness');

const CONFIG_PATH = path.join(__dirname, 'fixtures', 'copilot-worker-config.json');
const FAKE_COPILOT = path.join(__dirname, 'fixtures', 'fake-copilot.js');

/** Install an executable literally named `copilot` into the harness bin dir (already on PATH). */
function installFakeCopilot(binDir) {
  const shim = path.join(binDir, 'copilot');
  fs.writeFileSync(shim, `#!/bin/sh\nexec node "${FAKE_COPILOT}" "$@"\n`, { mode: 0o755 });
  fs.chmodSync(shim, 0o755);
}

function gitCommitFile(repoDir, relPath, content, message) {
  const absPath = path.join(repoDir, relPath);
  fs.mkdirSync(path.dirname(absPath), { recursive: true });
  fs.writeFileSync(absPath, content);
  for (const args of [
    ['add', relPath],
    ['commit', '-m', message],
  ]) {
    const result = spawnSync('git', args, { cwd: repoDir, encoding: 'utf8' });
    if (result.status !== 0) {
      throw new Error(`git ${args.join(' ')} failed: ${result.stderr || result.stdout}`);
    }
  }
}

describe('e2e: copilot provider', function () {
  this.timeout(60000);

  let env;

  beforeEach(() => {
    env = setupE2ERepo();
    installFakeCopilot(env.binDir);
  });

  afterEach(() => {
    cleanupE2ERepo(env);
  });

  it('runs a cluster through the copilot provider and writes into the worktree', async function () {
    const issueDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-e2e-issue-'));
    const issuePath = path.join(issueDir, 'feature.md');
    fs.writeFileSync(issuePath, '# Add feature\n\nDo X.\n');

    const clusterId = 'e2e-copilot-worker';
    const result = runZeroshot(
      env,
      ['run', issuePath, '--worktree', '--provider', 'copilot', '--config', CONFIG_PATH],
      { ZEROSHOT_CLUSTER_ID: clusterId }
    );

    assert.strictEqual(
      result.status,
      0,
      `zeroshot run exited ${result.status}\nSTDOUT:\n${result.stdout}\nSTDERR:\n${result.stderr}`
    );

    const cluster = await waitForClusterState(env, clusterId, ['stopped', 'killed']);
    assert.strictEqual(
      cluster.state,
      'stopped',
      'cluster should stop cleanly via the completion hook'
    );

    // The fake copilot's file write lands in the worktree (proves cwd injection + real spawn).
    const worktreeDir = worktreePath(env, clusterId);
    const writtenFile = path.join(worktreeDir, 'output.txt');
    assert.ok(fs.existsSync(writtenFile), `expected ${writtenFile} to exist`);
    assert.strictEqual(fs.readFileSync(writtenFile, 'utf8'), 'copilot implemented\n');

    // Nothing leaks into the main checkout.
    assert.ok(
      !fs.existsSync(path.join(env.repoDir, 'output.txt')),
      'output.txt must not leak into the main checkout'
    );
    assert.strictEqual(gitStatusPorcelain(env.repoDir), '', 'main checkout should remain clean');

    // The completion hook only fires if the copilot JSONL `result` event was parsed as success,
    // proving the copilot adapter/parser ran end to end (not just that a subprocess executed).
    const completions = readLedgerMessages(env, clusterId, 'TASK_COMPLETE');
    assert.ok(
      completions.length >= 1,
      'expected a TASK_COMPLETE message from the worker onComplete hook'
    );

    fs.rmSync(issueDir, { recursive: true, force: true });
  });

  it('forwards the repo .mcp.json to copilot as --additional-mcp-config', async function () {
    // The repo's `.claude/.mcp.json` (the same MCP source Claude consumes) must be committed so it
    // lands in the HEAD-based worktree the worker runs in.
    const mcpJson = JSON.stringify({
      mcpServers: { demo: { command: 'demo-mcp-bin', args: ['--stdio'] } },
    });
    gitCommitFile(env.repoDir, path.join('.claude', '.mcp.json'), mcpJson, 'Add MCP config');

    const issueDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-e2e-issue-'));
    const issuePath = path.join(issueDir, 'feature.md');
    fs.writeFileSync(issuePath, '# Add feature\n\nDo X.\n');

    const clusterId = 'e2e-copilot-mcp';
    const result = runZeroshot(
      env,
      ['run', issuePath, '--worktree', '--provider', 'copilot', '--config', CONFIG_PATH],
      { ZEROSHOT_CLUSTER_ID: clusterId }
    );

    assert.strictEqual(
      result.status,
      0,
      `zeroshot run exited ${result.status}\nSTDOUT:\n${result.stdout}\nSTDERR:\n${result.stderr}`
    );

    const cluster = await waitForClusterState(env, clusterId, ['stopped', 'killed']);
    assert.strictEqual(cluster.state, 'stopped', 'cluster should stop cleanly');

    // The fake copilot records the exact argv it was spawned with, into the worktree cwd.
    const worktreeDir = worktreePath(env, clusterId);
    const argvLog = path.join(worktreeDir, 'copilot-received-argv.json');
    assert.ok(fs.existsSync(argvLog), `expected ${argvLog} to exist`);

    const argv = JSON.parse(fs.readFileSync(argvLog, 'utf8'));
    const flagIndex = argv.indexOf('--additional-mcp-config');
    assert.ok(
      flagIndex >= 0,
      `expected copilot to receive --additional-mcp-config; got argv: ${JSON.stringify(argv)}`
    );
    // The repo .mcp.json content is inlined verbatim as the flag value (no @path, no translation).
    assert.strictEqual(
      argv[flagIndex + 1],
      mcpJson,
      'the inlined MCP config value must equal the repo .mcp.json content'
    );

    fs.rmSync(issueDir, { recursive: true, force: true });
  });
});
