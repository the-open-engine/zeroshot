/**
 * Focused Docker-isolation proof for Copilot MCP config.
 *
 * The heavyweight `zeroshot run --docker` path (tests/integration/e2e-isolation-and-auto.test.sh)
 * needs the real `zeroshot-cluster-base` image plus live Claude/Copilot credentials — none are
 * available offline, and COPILOT_GITHUB_TOKEN is unset, so a live Copilot API call is impossible.
 *
 * This proves the Docker-specific piece for the MCP feature: the copilot adapter's inlined
 * `--additional-mcp-config <json>` argument survives the container boundary intact. It builds the
 * REAL copilot argv on the host with the compiled adapter (reading the repo `.mcp.json`, exactly as
 * the agent spawn path does), then delivers it into a container via `docker exec -i <container>
 * <command>` — the exact mechanism IsolationManager.spawnInContainer uses. A fake `copilot` inside
 * the container records the argv it received; the host asserts the inlined config arrived unchanged.
 *
 * Fully offline: no zeroshot-cluster-base image, no credentials, no Copilot API call.
 *
 * REQUIRES: Docker installed and running, and the node:20-slim image already pulled. SKIPS
 * otherwise (or under CI) — the base image pull is left to the environment to avoid slow network
 * pulls inside a test hook.
 */

const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawnSync } = require('child_process');
const IsolationManager = require('../../src/isolation-manager');
const { prepareSingleAgentProviderCommand } = require('../../task-lib/provider-helper-runtime.js');

const IMAGE = 'node:20-slim';
const REPO_ROOT = path.resolve(__dirname, '..', '..');
const FAKE_COPILOT = path.join(REPO_ROOT, 'tests', 'e2e', 'fixtures', 'fake-copilot.js');
const MCP_JSON = JSON.stringify({
  mcpServers: { demo: { command: 'demo-mcp-bin', args: ['--stdio'] } },
});

function dockerCli(args, opts = {}) {
  return spawnSync('docker', args, { encoding: 'utf8', ...opts });
}

function imagePresent(image) {
  return dockerCli(['image', 'inspect', image]).status === 0;
}

describe('copilot MCP config under Docker isolation', function () {
  this.timeout(120000);

  let ctxDir;
  let container;
  let hostArgs;

  before(function () {
    if (process.env.CI || !IsolationManager.isDockerAvailable() || !imagePresent(IMAGE)) {
      this.skip();
    }
  });

  beforeEach(function () {
    ctxDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-copilot-mcp-docker-'));
    fs.writeFileSync(path.join(ctxDir, '.mcp.json'), MCP_JSON);
    fs.copyFileSync(FAKE_COPILOT, path.join(ctxDir, 'fake-copilot.js'));
    const shim = path.join(ctxDir, 'copilot');
    fs.writeFileSync(shim, '#!/bin/sh\nexec node "$(dirname "$0")/fake-copilot.js" "$@"\n', {
      mode: 0o755,
    });
    fs.chmodSync(shim, 0o755);

    // Build the copilot argv on the host exactly as the agent spawn path does: the repo .mcp.json
    // content is inlined into options.mcpConfig, and the copilot adapter emits it verbatim.
    const content = fs.readFileSync(path.join(ctxDir, '.mcp.json'), 'utf8').trim();
    const prepared = prepareSingleAgentProviderCommand({
      provider: 'copilot',
      context: 'do the work',
      options: {
        autoApprove: true,
        outputFormat: 'json',
        cwd: '/app',
        mcpConfig: [content],
        cliFeatures: {
          supportsJsonOutput: true,
          supportsAllowAll: true,
          supportsNoAskUser: true,
          supportsAddDir: true,
          supportsMcpConfig: true,
        },
      },
    });
    hostArgs = prepared.commandSpec.args;
    container = null;
  });

  afterEach(function () {
    if (container) {
      dockerCli(['rm', '-f', container], { stdio: 'pipe' });
      container = null;
    }
    if (ctxDir) {
      fs.rmSync(ctxDir, { recursive: true, force: true });
    }
  });

  it('delivers --additional-mcp-config to copilot intact through docker exec', function () {
    // Sanity: the host-built argv must carry the inlined MCP config (else the container proof is
    // vacuous).
    const hostFlag = hostArgs.indexOf('--additional-mcp-config');
    assert.ok(
      hostFlag >= 0 && hostArgs[hostFlag + 1] === MCP_JSON,
      'adapter did not emit MCP flag'
    );

    const run = dockerCli(['run', '-d', IMAGE, 'tail', '-f', '/dev/null']);
    assert.strictEqual(run.status, 0, `docker run failed: ${run.stderr}`);
    container = run.stdout.trim();

    assert.strictEqual(dockerCli(['exec', container, 'mkdir', '-p', '/app']).status, 0);
    const cp = dockerCli(['cp', `${ctxDir}/.`, `${container}:/app`]);
    assert.strictEqual(cp.status, 0, `docker cp failed: ${cp.stderr}`);
    dockerCli(['exec', container, 'chmod', '+x', '/app/copilot']);

    // Mirror IsolationManager.spawnInContainer: `docker exec -i <container> <command>` with the
    // provider argv the host built. -w /app makes the fake copilot record argv under /app.
    const exec = dockerCli([
      'exec',
      '-i',
      '-w',
      '/app',
      '-e',
      'PATH=/app:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin',
      container,
      'copilot',
      ...hostArgs,
    ]);
    assert.strictEqual(
      exec.status,
      0,
      `copilot failed inside container:\nSTDOUT:\n${exec.stdout}\nSTDERR:\n${exec.stderr}`
    );

    const out = dockerCli(['exec', container, 'cat', '/app/copilot-received-argv.json']);
    assert.strictEqual(out.status, 0, `could not read recorded argv: ${out.stderr}`);
    const argv = JSON.parse(out.stdout);

    // The argv the fake copilot received inside the container must match what the host sent, with
    // the inlined MCP config unchanged.
    assert.deepStrictEqual(argv, hostArgs, 'argv mutated crossing the container boundary');
    const flagIndex = argv.indexOf('--additional-mcp-config');
    assert.ok(
      flagIndex >= 0,
      `no --additional-mcp-config in container argv: ${JSON.stringify(argv)}`
    );
    assert.strictEqual(argv[flagIndex + 1], MCP_JSON);
  });
});
