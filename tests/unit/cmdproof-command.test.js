const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');

const {
  buildCmdproofArgs,
  parseCommandToArgv,
  proofLockName,
  runCmdproof,
} = require('../../cli/commands/cmdproof');

describe('zeroshot cmdproof command', function () {
  it('parses configured command strings into argv', function () {
    assert.deepStrictEqual(parseCommandToArgv('bash ./scripts/ci/run-local-ci-equivalent.sh'), [
      'bash',
      './scripts/ci/run-local-ci-equivalent.sh',
    ]);
    assert.deepStrictEqual(parseCommandToArgv('npm run "test:unit"'), ['npm', 'run', 'test:unit']);
  });

  it('builds cmdproof prove and verify argv for a proof', function () {
    const proof = {
      id: 'opcore-ci',
      profile: 'ci-equivalent',
      command: 'bash ./scripts/ci/run-local-ci-equivalent.sh',
    };
    const paths = {
      cacheDir: '/tmp/cache',
      privateKeyPath: '/tmp/keys/private-key.json',
      publicKeyPath: '/tmp/keys/public-key.json',
    };

    assert.deepStrictEqual(buildCmdproofArgs('prove', proof, paths), [
      'prove',
      '--profile',
      'ci-equivalent',
      '--cas',
      '/tmp/cache',
      '--fallback',
      'run',
      '--signing-key',
      '/tmp/keys/private-key.json',
      '--',
      'bash',
      './scripts/ci/run-local-ci-equivalent.sh',
    ]);

    assert.deepStrictEqual(buildCmdproofArgs('verify', proof, paths), [
      'verify',
      '--profile',
      'ci-equivalent',
      '--cas',
      '/tmp/cache',
      '--trusted-key',
      '/tmp/keys/public-key.json',
      '--',
      'bash',
      './scripts/ci/run-local-ci-equivalent.sh',
    ]);
  });

  it('ensures keys and runs prove from environment command proofs', function () {
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-cmdproof-test-'));
    const calls = [];
    const env = {
      ZEROSHOT_CLUSTER_ID: 'cluster-1',
      ZEROSHOT_COMMAND_PROOFS: JSON.stringify([
        {
          id: 'opcore-ci',
          profile: 'ci-equivalent',
          command: 'bash ./scripts/ci/run-local-ci-equivalent.sh',
        },
      ]),
      CMDPROOF_CACHE_DIR: path.join(tempDir, 'cache'),
      CMDPROOF_KEY_DIR: path.join(tempDir, 'keys'),
    };

    try {
      const exitCode = runCmdproof({
        mode: 'prove',
        id: 'opcore-ci',
        env,
        cwd: tempDir,
        spawnSyncFn: (command, args) => {
          calls.push({ command, args });
          if (args[0] === 'keygen') {
            fs.writeFileSync(path.join(tempDir, 'keys', 'private-key.json'), '{}');
            fs.writeFileSync(path.join(tempDir, 'keys', 'public-key.json'), '{}');
          }
          return { status: 0, stdout: '', stderr: '' };
        },
      });

      assert.strictEqual(exitCode, 0);
      assert.strictEqual(calls[0].args[0], 'keygen');
      assert.deepStrictEqual(calls[1].args.slice(0, 2), ['prove', '--profile']);
    } finally {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  it('falls back to prove when check misses verification', function () {
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-cmdproof-test-'));
    const calls = [];
    const env = {
      ZEROSHOT_COMMAND_PROOFS: JSON.stringify([
        {
          id: 'opcore-ci',
          profile: 'ci-equivalent',
          command: 'bash ./scripts/ci/run-local-ci-equivalent.sh',
        },
      ]),
      CMDPROOF_CACHE_DIR: path.join(tempDir, 'cache'),
      CMDPROOF_KEY_DIR: path.join(tempDir, 'keys'),
      ZEROSHOT_CMDPROOF_LOCK_DIR: path.join(tempDir, 'locks'),
    };

    try {
      fs.mkdirSync(env.CMDPROOF_KEY_DIR, { recursive: true });
      fs.writeFileSync(path.join(env.CMDPROOF_KEY_DIR, 'private-key.json'), '{}');
      fs.writeFileSync(path.join(env.CMDPROOF_KEY_DIR, 'public-key.json'), '{}');

      const exitCode = runCmdproof({
        mode: 'check',
        id: 'opcore-ci',
        env,
        cwd: tempDir,
        spawnSyncFn: (command, args) => {
          calls.push({ command, args });
          if (args[0] === 'verify') {
            return { status: 2, stdout: '{"status":"miss"}\n', stderr: '' };
          }
          return { status: 0, stdout: '', stderr: '' };
        },
        stdout: { write: () => {} },
        stderr: { write: () => {} },
      });

      assert.strictEqual(exitCode, 0);
      assert.deepStrictEqual(
        calls.map((call) => call.args[0]),
        ['verify', 'prove']
      );
      assert.strictEqual(fs.existsSync(env.ZEROSHOT_CMDPROOF_LOCK_DIR), false);
    } finally {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  it('waits for an in-flight proof and reuses it instead of proving concurrently', function () {
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-cmdproof-test-'));
    const proof = {
      id: 'opcore-ci',
      profile: 'ci-equivalent',
      command: 'bash ./scripts/ci/run-local-ci-equivalent.sh',
    };
    const env = {
      ZEROSHOT_COMMAND_PROOFS: JSON.stringify([proof]),
      CMDPROOF_CACHE_DIR: path.join(tempDir, 'cache'),
      CMDPROOF_KEY_DIR: path.join(tempDir, 'keys'),
      ZEROSHOT_CMDPROOF_LOCK_DIR: path.join(tempDir, 'locks'),
      ZEROSHOT_CMDPROOF_WAIT_MS: '50',
      ZEROSHOT_CMDPROOF_POLL_MS: '1',
    };
    const calls = [];
    const stdout = [];
    const stderr = [];

    try {
      fs.mkdirSync(env.CMDPROOF_KEY_DIR, { recursive: true });
      fs.writeFileSync(path.join(env.CMDPROOF_KEY_DIR, 'private-key.json'), '{}');
      fs.writeFileSync(path.join(env.CMDPROOF_KEY_DIR, 'public-key.json'), '{}');
      const lockPath = path.join(
        env.ZEROSHOT_CMDPROOF_LOCK_DIR,
        proofLockName(proof, { actionKey: 'action-a' })
      );
      fs.mkdirSync(lockPath, { recursive: true });
      fs.writeFileSync(path.join(lockPath, 'owner.json'), JSON.stringify({ pid: process.pid }));

      const exitCode = runCmdproof({
        mode: 'check',
        id: 'opcore-ci',
        env,
        cwd: tempDir,
        spawnSyncFn: (command, args) => {
          calls.push({ command, args });
          if (
            args[0] === 'verify' &&
            calls.filter((call) => call.args[0] === 'verify').length === 1
          ) {
            return { status: 2, stdout: '{"status":"miss","actionKey":"action-a"}\n', stderr: '' };
          }
          if (args[0] === 'verify') {
            return { status: 0, stdout: '{"status":"reused_proof","reused":true}\n', stderr: '' };
          }
          return { status: 0, stdout: '', stderr: '' };
        },
        stdout: { write: (chunk) => stdout.push(chunk) },
        stderr: { write: (chunk) => stderr.push(chunk) },
      });

      assert.strictEqual(exitCode, 0);
      assert.deepStrictEqual(
        calls.map((call) => call.args[0]),
        ['verify', 'verify']
      );
      assert.match(stdout.join(''), /"status":"miss"/);
      assert.match(stdout.join(''), /"status":"reused_proof"/);
      assert.match(stderr.join(''), /waiting for in-flight proof/);
    } finally {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  it('takes over proving when an in-flight proof lock is stale', function () {
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-cmdproof-test-'));
    const proof = {
      id: 'opcore-ci',
      profile: 'ci-equivalent',
      command: 'bash ./scripts/ci/run-local-ci-equivalent.sh',
    };
    const env = {
      ZEROSHOT_COMMAND_PROOFS: JSON.stringify([proof]),
      CMDPROOF_CACHE_DIR: path.join(tempDir, 'cache'),
      CMDPROOF_KEY_DIR: path.join(tempDir, 'keys'),
      ZEROSHOT_CMDPROOF_LOCK_DIR: path.join(tempDir, 'locks'),
      ZEROSHOT_CMDPROOF_LOCK_STALE_MS: '1',
    };
    const calls = [];

    try {
      fs.mkdirSync(env.CMDPROOF_KEY_DIR, { recursive: true });
      fs.writeFileSync(path.join(env.CMDPROOF_KEY_DIR, 'private-key.json'), '{}');
      fs.writeFileSync(path.join(env.CMDPROOF_KEY_DIR, 'public-key.json'), '{}');
      const lockPath = path.join(
        env.ZEROSHOT_CMDPROOF_LOCK_DIR,
        proofLockName(proof, { actionKey: 'action-stale' })
      );
      fs.mkdirSync(lockPath, { recursive: true });
      const old = new Date(Date.now() - 10_000);
      fs.utimesSync(lockPath, old, old);

      const exitCode = runCmdproof({
        mode: 'check',
        id: 'opcore-ci',
        env,
        cwd: tempDir,
        spawnSyncFn: (command, args) => {
          calls.push({ command, args });
          if (args[0] === 'verify') {
            return {
              status: 2,
              stdout: '{"status":"miss","actionKey":"action-stale"}\n',
              stderr: '',
            };
          }
          return { status: 0, stdout: '{"status":"passed"}\n', stderr: '' };
        },
        stdout: { write: () => {} },
        stderr: { write: () => {} },
      });

      assert.strictEqual(exitCode, 0);
      assert.deepStrictEqual(
        calls.map((call) => call.args[0]),
        ['verify', 'prove']
      );
      assert.strictEqual(fs.existsSync(lockPath), false);
    } finally {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });
});
