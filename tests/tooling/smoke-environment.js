'use strict';

// Real direct-target lifecycle smoke. Uses terminal graphs, never provider credentials.
const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const { randomBytes } = require('node:crypto');
const { setTimeout: delay } = require('node:timers/promises');

function uuidV7() {
  const bytes = randomBytes(16);
  bytes.writeUIntBE(Date.now(), 0, 6);
  bytes[6] = (bytes[6] & 15) | 112;
  bytes[8] = (bytes[8] & 63) | 128;
  const hex = bytes.toString('hex');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

async function until(check, label, timeout = 120_000) {
  const deadline = Date.now() + timeout;
  for (;;) {
    const result = await check();
    if (result) return result;
    assert.ok(Date.now() < deadline, `Timed out waiting for ${label}`);
    await delay(100);
  }
}

async function connect(origin) {
  await until(
    async () => {
      try {
        return (
          await fetch(new URL('/.well-known/zeroshot-native-v2', origin), {
            signal: AbortSignal.timeout(1000),
          })
        ).ok;
      } catch {
        return false;
      }
    },
    'target readiness',
    20_000
  );
  const socket = new WebSocket(new URL('/native-v2/oecp', origin).href.replace(/^http/, 'ws'));
  const pending = new Map();
  const logs = new Map();
  let sequence = 0;
  socket.addEventListener('message', ({ data }) => {
    const message = JSON.parse(data);
    const waiter = pending.get(message.id);
    if (waiter) {
      pending.delete(message.id);
      clearTimeout(waiter.timer);
      if (message.error) waiter.reject(new Error(JSON.stringify(message.error)));
      else waiter.resolve(message.result);
    } else if (message.params?.record) {
      const { runId, record } = message.params;
      logs.set(runId, [...(logs.get(runId) || []), record.message]);
    }
  });
  await new Promise((resolve, reject) => {
    socket.addEventListener('open', resolve, { once: true });
    socket.addEventListener('error', reject, { once: true });
  });
  const rpc = (method, params) =>
    new Promise((resolve, reject) => {
      const id = ++sequence;
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`${method} timed out`));
      }, 20_000);
      pending.set(id, { resolve, reject, timer });
      socket.send(JSON.stringify({ jsonrpc: '2.0', id, method, params }));
    });
  await rpc('initialize', { protocolVersion: 'openengine.cluster/v1' });
  return { rpc, logs, close: () => socket.close() };
}

function submission(revision, environment, root) {
  const runId = uuidV7();
  return {
    runId,
    submission: {
      title: 'Environment Docker smoke',
      graph: {
        profile: 'openengine.graph.full/v1',
        initialInput: { kind: 'null' },
        policy: { policy: 'policy.native-v2@1', default: 'deny' },
        root: root || { kind: 'succeed', name: 'done', output: { kind: 'null' }, bindings: [] },
      },
      initialInput: null,
      runtime: { harness: 'codex', provider: 'openai', size: 'small', nodes: {} },
      environment,
      source: { repository: 'the-open-engine/zeroshot', branch: 'main', revision },
      submissionKey: `environment-smoke-${runId}`,
    },
    connections: {},
  };
}

async function submit(origin, request) {
  const started = Date.now();
  const response = await fetch(new URL('/native-v2/run', origin), {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(request),
    signal: AbortSignal.timeout(5000),
  });
  const result = await response.json();
  assert.equal(response.status, 200, JSON.stringify(result));
  assert.equal(result.runId, request.runId);
  assert.ok(Date.now() - started < 3000, 'submission must not wait for environment preparation');
}

async function observedSubmit(context, request) {
  await submit(context.origin, request);
  await context.client.rpc('run/logs', { runId: request.runId });
}

async function terminal(client, runId) {
  return until(async () => {
    const result = await client.rpc('run/status', { runId });
    return result.status.phase === 'finished' && result;
  }, `run ${runId} settlement`);
}

async function successful(context, request) {
  await observedSubmit(context, request);
  const result = await terminal(context.client, request.runId);
  assert.equal(result.status.terminalResult.status, 'succeeded', JSON.stringify(result));
}

async function expectLog(client, runId, marker) {
  await until(() => client.logs.get(runId)?.some((line) => line.includes(marker)), marker);
}

function messages(client, runId) {
  return (client.logs.get(runId) || []).join('\n');
}

function proofAbsent(container, path) {
  execFileSync('docker', ['exec', '--workdir', '/', container, 'test', '!', '-e', path]);
}

async function success(context) {
  const request = submission(context.revision, {
    variables: { SMOKE_LABEL: 'declared' },
    connections: { smoke: ['SMOKE_SECRET'] },
    setup: [
      'test "$(id -u)" = 0',
      'test ! -d workspace/.git',
      'printf \'#!/bin/sh\\necho shared-tool-ready\\n\' > "$ZEROSHOT_TOOLS/bin/environment-proof"',
      'chmod 0755 "$ZEROSHOT_TOOLS/bin/environment-proof"',
      'echo setup-once',
    ].join('\n'),
    startup: [
      'test "$(id -u)" -ne 0',
      'test -d .git',
      'test "$SMOKE_LABEL" = declared',
      'environment-proof',
      'printf \'%s\\n\' "$SMOKE_SECRET"',
      'echo startup-once',
    ].join('\n'),
  });
  const secret = `never-publish-${randomBytes(16).toString('hex')}`;
  request.connections = { smoke: { SMOKE_SECRET: secret } };
  await successful(context, request);
  await expectLog(context.client, request.runId, 'startup-once');
  const log = messages(context.client, request.runId);
  assert.equal(log.match(/setup-once/g)?.length, 1);
  assert.equal(log.match(/startup-once/g)?.length, 1);
  assert.match(log, /shared-tool-ready/);
  assert.ok(!log.includes(secret), 'environment connection values must be redacted');
  process.stdout.write('Environment setup/startup, shared tools and redaction passed\n');
}

async function failure(context, phase) {
  const request = submission(context.revision, {
    setup: phase === 'setup' ? 'echo deliberate-setup-failure; exit 23' : 'echo setup-ready',
    startup:
      phase === 'startup' ? 'echo deliberate-startup-failure; exit 24' : 'echo forbidden-startup',
  });
  await observedSubmit(context, request);
  const result = await terminal(context.client, request.runId);
  assert.equal(
    result.status.terminalResult.reason,
    `environment_${phase}_failed`,
    JSON.stringify(result)
  );
  await expectLog(context.client, request.runId, `deliberate-${phase}-failure`);
  assert.ok(!messages(context.client, request.runId).includes('forbidden-startup'));
  process.stdout.write(`Environment ${phase} failure settled without graph execution\n`);
}

async function cancellation(context) {
  const proof = `/tmp/environment-cancel-${uuidV7()}`;
  const request = submission(context.revision, {
    setup: `echo blocked-setup\n(sleep 4; touch '${proof}') &\nwait`,
    startup: 'echo forbidden-startup-after-cancel',
  });
  await observedSubmit(context, request);
  await expectLog(context.client, request.runId, 'blocked-setup');
  await submit(context.origin, request);
  await context.client.rpc('run/force', { runId: request.runId });
  const result = await terminal(context.client, request.runId);
  assert.equal(result.status.terminalResult.reason, 'force_stopped', JSON.stringify(result));
  await delay(4500);
  proofAbsent(context.container, proof);
  const log = messages(context.client, request.runId);
  assert.equal(log.match(/blocked-setup/g)?.length, 1, 'replay must not start another hook');
  assert.ok(!log.includes('forbidden-startup-after-cancel'));
  process.stdout.write('Preparation replay and cancellation cleaned up descendants\n');
}

async function resume(context) {
  const request = submission(
    context.revision,
    {
      setup: 'echo setup-per-attempt',
      startup: [
        'count=$(cat .environment-startup-count 2>/dev/null || echo 0)',
        'printf \'%s\\n\' "$((count + 1))" > .environment-startup-count',
        'printf \'startup-attempt=%s\\n\' "$(cat .environment-startup-count)"',
      ].join('\n'),
    },
    { kind: 'fail', name: 'failed', reason: 'fixture_failed' }
  );
  await observedSubmit(context, request);
  const initial = await terminal(context.client, request.runId);
  assert.equal(initial.status.terminalResult.reason, 'fixture_failed', JSON.stringify(initial));
  assert.equal(initial.workspaceRecovery?.recoverable, true, JSON.stringify(initial));
  const successorRunId = uuidV7();
  await context.client.rpc('run/resume', { runId: request.runId, successorRunId, connections: {} });
  await context.client.rpc('run/logs', { runId: successorRunId });
  const successor = await terminal(context.client, successorRunId);
  assert.equal(successor.status.terminalResult.reason, 'fixture_failed', JSON.stringify(successor));
  await expectLog(context.client, successorRunId, 'startup-attempt=2');
  for (const runId of [request.runId, successorRunId]) {
    assert.equal(messages(context.client, runId).match(/setup-per-attempt/g)?.length, 1);
  }
  process.stdout.write('Resume reran setup and startup around restored workspace files\n');
}

async function dockerWorkload(context, image) {
  assert.match(image, /^[a-zA-Z0-9/:._@-]+$/);
  const name = `environment-child-${uuidV7()}`;
  const request = submission(context.revision, {
    variables: { DOCKER_HOST: 'unix:///var/run/docker.sock' },
    startup: [
      'test "$(id -u)" -ne 0',
      'docker version --format "{{.Server.Version}}"',
      `docker run --rm --network none --name '${name}' --entrypoint /bin/sh '${image}' ` +
        "-c 'echo docker-child-ready'",
    ].join('\n'),
  });
  await successful(context, request);
  await expectLog(context.client, request.runId, 'docker-child-ready');
  process.stdout.write('Non-root startup used Docker to create and remove a workload\n');
}

async function main() {
  const args = process.argv.slice(2);
  const options = Object.fromEntries(
    args.reduce((pairs, value, index) => {
      if (index % 2 === 0) pairs.push([value, args[index + 1]]);
      return pairs;
    }, [])
  );
  const origin = options['--url'];
  const revision = options['--revision'];
  const container = options['--container'];
  assert.ok(
    origin && container && /^[a-f0-9]{40}$/.test(revision),
    'usage: smoke-environment.js --url ORIGIN --revision COMMIT --container NAME'
  );
  const client = await connect(origin);
  const context = { origin, revision, container, client };
  try {
    await success(context);
    await failure(context, 'setup');
    await failure(context, 'startup');
    await cancellation(context);
    await resume(context);
    if (options['--docker-image']) await dockerWorkload(context, options['--docker-image']);
  } finally {
    client.close();
  }
}

main().catch((error) => {
  process.stderr.write(`${error.stack || String(error)}\n`);
  process.exitCode = 1;
});
