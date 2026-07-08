const fs = require('fs');
const os = require('os');
const path = require('path');
const crypto = require('crypto');
const { spawnSync } = require('child_process');

const {
  normalizeCommandProofs,
  resolveConfiguredCommandProofs,
} = require('../../src/command-proofs');

function pushCurrentArg(state) {
  if (!state.current) {
    return;
  }
  state.args.push(state.current);
  state.current = '';
}

function consumeEscapedChar(state, char) {
  state.current += char;
  state.escaped = false;
}

function consumeQuotedChar(state, char) {
  if (char === state.quote) {
    state.quote = null;
    return;
  }
  state.current += char;
}

function consumeUnquotedChar(state, char) {
  if (char === '\\') {
    state.escaped = true;
    return;
  }
  if (char === '"' || char === "'") {
    state.quote = char;
    return;
  }
  if (/\s/.test(char)) {
    pushCurrentArg(state);
    return;
  }
  state.current += char;
}

function consumeCommandChar(state, char) {
  if (state.escaped) {
    consumeEscapedChar(state, char);
    return;
  }
  if (state.quote) {
    consumeQuotedChar(state, char);
    return;
  }
  consumeUnquotedChar(state, char);
}

function parseCommandToArgv(command) {
  if (typeof command !== 'string' || command.trim() === '') {
    return [];
  }

  const state = { args: [], current: '', quote: null, escaped: false };

  for (const char of command.trim()) {
    consumeCommandChar(state, char);
  }

  if (state.escaped) {
    state.current += '\\';
  }
  if (state.quote) {
    throw new Error(`Unterminated quote in command: ${command}`);
  }
  pushCurrentArg(state);
  return state.args;
}

function buildCmdproofArgs(mode, proof, paths) {
  const argv = parseCommandToArgv(proof.command);
  if (argv.length === 0) {
    throw new Error(`Command proof ${proof.id} has an empty command`);
  }

  if (mode === 'prove') {
    return [
      'prove',
      '--profile',
      proof.profile,
      '--cas',
      paths.cacheDir,
      '--fallback',
      'run',
      '--signing-key',
      paths.privateKeyPath,
      '--',
      ...argv,
    ];
  }

  if (mode === 'verify') {
    return [
      'verify',
      '--profile',
      proof.profile,
      '--cas',
      paths.cacheDir,
      '--trusted-key',
      paths.publicKeyPath,
      '--',
      ...argv,
    ];
  }

  throw new Error(`Unsupported cmdproof mode: ${mode}`);
}

function parseEnvProofs(env) {
  if (!env.ZEROSHOT_COMMAND_PROOFS) {
    return [];
  }
  try {
    return normalizeCommandProofs(JSON.parse(env.ZEROSHOT_COMMAND_PROOFS));
  } catch (error) {
    throw new Error(`Invalid ZEROSHOT_COMMAND_PROOFS JSON: ${error.message}`);
  }
}

function resolveProofs(env, cwd) {
  const envProofs = parseEnvProofs(env);
  if (envProofs.length > 0) {
    return envProofs;
  }
  return resolveConfiguredCommandProofs({}, { cwd });
}

function resolveProof(id, env, cwd) {
  const proofs = resolveProofs(env, cwd);
  const proof = proofs.find((candidate) => candidate.id === id);
  if (!proof) {
    throw new Error(`Unknown command proof id "${id}"`);
  }
  return proof;
}

function resolvePaths(env) {
  const clusterId = env.ZEROSHOT_CLUSTER_ID || 'local';
  const root = path.join(os.homedir(), '.zeroshot', 'cmdproof', clusterId);
  const cacheDir = env.CMDPROOF_CACHE_DIR || path.join(root, 'cache');
  const keyDir = env.CMDPROOF_KEY_DIR || path.join(root, 'keys');
  return {
    cacheDir,
    keyDir,
    lockDir: env.ZEROSHOT_CMDPROOF_LOCK_DIR || path.join(root, 'locks'),
    privateKeyPath: path.join(keyDir, 'private-key.json'),
    publicKeyPath: path.join(keyDir, 'public-key.json'),
  };
}

function statusOf(result) {
  if (typeof result.status === 'number') {
    return result.status;
  }
  if (result.error) {
    return 127;
  }
  return 1;
}

function spawnCmdproof(args, options) {
  const result = options.spawnSyncFn('cmdproof', args, {
    cwd: options.cwd,
    env: options.env,
    encoding: 'utf8',
  });
  if (result.error && result.error.code === 'ENOENT') {
    throw new Error('cmdproof binary not found on PATH');
  }
  return result;
}

function parsePositiveInt(value, fallback) {
  const parsed = Number.parseInt(String(value || ''), 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function sleepMs(ms) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

function proofLockName(proof, verifyReport = null) {
  const actionKey = getVerifyActionKey(verifyReport);
  const digest = crypto
    .createHash('sha256')
    .update(`${proof.id}\0${proof.profile}\0${proof.command}\0${actionKey}`)
    .digest('hex')
    .slice(0, 16);
  const safeId = proof.id.replace(/[^a-zA-Z0-9._-]/g, '_');
  return `${safeId}-${digest}.lock`;
}

function getVerifyActionKey(verifyReport) {
  return verifyReport?.actionKey || verifyReport?.action_key || '';
}

function proofLockPath(proof, paths, verifyReport = null) {
  return path.join(paths.lockDir, proofLockName(proof, verifyReport));
}

function removeLock(lockPath) {
  fs.rmSync(lockPath, { recursive: true, force: true });
}

function lockOwnerState(lockPath) {
  try {
    const metadata = JSON.parse(fs.readFileSync(path.join(lockPath, 'owner.json'), 'utf8'));
    if (!Number.isInteger(metadata.pid) || metadata.pid <= 0) {
      return 'unknown';
    }
    try {
      process.kill(metadata.pid, 0);
      return 'alive';
    } catch (error) {
      return error?.code === 'ESRCH' ? 'gone' : 'unknown';
    }
  } catch {
    return 'unknown';
  }
}

function lockIsStale(lockPath, staleMs) {
  try {
    const stat = fs.statSync(lockPath);
    const ownerState = lockOwnerState(lockPath);
    if (ownerState === 'alive') {
      return false;
    }
    if (ownerState === 'gone') {
      return true;
    }
    return Date.now() - stat.mtimeMs > staleMs;
  } catch {
    return false;
  }
}

function writeLockMetadata(lockPath, proof) {
  const metadata = {
    pid: process.pid,
    proofId: proof.id,
    profile: proof.profile,
    command: proof.command,
    createdAt: new Date().toISOString(),
  };
  fs.writeFileSync(path.join(lockPath, 'owner.json'), `${JSON.stringify(metadata, null, 2)}\n`, {
    mode: 0o600,
  });
}

function tryAcquireProofLock(proof, paths, env, verifyReport = null) {
  const lockPath = proofLockPath(proof, paths, verifyReport);
  const staleMs = parsePositiveInt(env.ZEROSHOT_CMDPROOF_LOCK_STALE_MS, 6 * 60 * 60 * 1000);
  fs.mkdirSync(paths.lockDir, { recursive: true, mode: 0o700 });

  try {
    fs.mkdirSync(lockPath, { mode: 0o700 });
    writeLockMetadata(lockPath, proof);
    return { acquired: true, lockPath };
  } catch (error) {
    if (error?.code !== 'EEXIST') {
      throw error;
    }
    if (!lockIsStale(lockPath, staleMs)) {
      return { acquired: false, lockPath };
    }
    removeLock(lockPath);
    try {
      fs.mkdirSync(lockPath, { mode: 0o700 });
      writeLockMetadata(lockPath, proof);
      return { acquired: true, lockPath };
    } catch (retryError) {
      if (retryError?.code === 'EEXIST') {
        return { acquired: false, lockPath };
      }
      throw retryError;
    }
  }
}

function ensureKeypair(paths, options) {
  fs.mkdirSync(paths.cacheDir, { recursive: true, mode: 0o700 });
  fs.mkdirSync(paths.keyDir, { recursive: true, mode: 0o700 });
  if (fs.existsSync(paths.privateKeyPath) && fs.existsSync(paths.publicKeyPath)) {
    return;
  }

  const result = spawnCmdproof(
    [
      'keygen',
      '--purpose',
      'trusted-agent',
      '--out',
      paths.privateKeyPath,
      '--public-out',
      paths.publicKeyPath,
    ],
    options
  );
  if (statusOf(result) !== 0) {
    throw new Error(`cmdproof keygen failed with exit code ${statusOf(result)}`);
  }
}

function writeResult(result, stdout, stderr) {
  if (result.stdout) stdout.write(result.stdout);
  if (result.stderr) stderr.write(result.stderr);
}

function parseJsonObject(text) {
  try {
    return JSON.parse(String(text || '').trim());
  } catch {
    return null;
  }
}

function shouldFallbackFromVerify(result) {
  const parsed = parseJsonObject(result.stdout);
  if (parsed?.status === 'reused_proof') {
    return false;
  }
  return statusOf(result) === 2;
}

function runMode(mode, proof, paths, options) {
  const result = spawnCmdproof(buildCmdproofArgs(mode, proof, paths), options);
  writeResult(result, options.stdout, options.stderr);
  return statusOf(result);
}

function runProveWithLock(proof, paths, options, lockPath) {
  try {
    return runMode('prove', proof, paths, options);
  } finally {
    removeLock(lockPath);
  }
}

function verifyProof(proof, paths, options, { write = true } = {}) {
  const result = spawnCmdproof(buildCmdproofArgs('verify', proof, paths), options);
  if (write) {
    writeResult(result, options.stdout, options.stderr);
  }
  return result;
}

function waitForProofOrProve(proof, paths, options, verifyReport) {
  const startedAt = Date.now();
  const waitMs = parsePositiveInt(options.env.ZEROSHOT_CMDPROOF_WAIT_MS, 30 * 60 * 1000);
  const pollMs = parsePositiveInt(options.env.ZEROSHOT_CMDPROOF_POLL_MS, 2500);

  while (Date.now() - startedAt < waitMs) {
    sleepMs(pollMs);
    const verifyResult = verifyProof(proof, paths, options, { write: false });
    const latestReport = parseJsonObject(verifyResult.stdout) || verifyReport;
    if (!shouldFallbackFromVerify(verifyResult)) {
      writeResult(verifyResult, options.stdout, options.stderr);
      return statusOf(verifyResult);
    }

    const lockReport = getVerifyActionKey(latestReport) ? latestReport : verifyReport;
    const lock = tryAcquireProofLock(proof, paths, options.env, lockReport);
    if (lock.acquired) {
      options.stderr.write(
        `zeroshot cmdproof check ${proof.id}: no reusable proof appeared; acquired proof lock after wait.\n`
      );
      return runProveWithLock(proof, paths, options, lock.lockPath);
    }
  }

  options.stderr.write(
    `zeroshot cmdproof check ${proof.id}: timed out waiting ${waitMs}ms for in-flight proof; leaving miss for caller.\n`
  );
  const finalVerify = verifyProof(proof, paths, options, { write: true });
  return statusOf(finalVerify);
}

function runCheck(proof, paths, options) {
  const verifyResult = verifyProof(proof, paths, options);
  if (!shouldFallbackFromVerify(verifyResult)) {
    return statusOf(verifyResult);
  }
  const verifyReport = parseJsonObject(verifyResult.stdout);
  if (!getVerifyActionKey(verifyReport)) {
    return runMode('prove', proof, paths, options);
  }

  const lock = tryAcquireProofLock(proof, paths, options.env, verifyReport);
  if (lock.acquired) {
    return runProveWithLock(proof, paths, options, lock.lockPath);
  }

  options.stderr.write(
    `zeroshot cmdproof check ${proof.id}: proof miss; waiting for in-flight proof from another agent.\n`
  );
  return waitForProofOrProve(proof, paths, options, verifyReport);
}

function runCmdproof({
  mode,
  id,
  env = process.env,
  cwd = process.cwd(),
  spawnSyncFn = spawnSync,
  stdout = process.stdout,
  stderr = process.stderr,
}) {
  if (!['prove', 'verify', 'check'].includes(mode)) {
    throw new Error(`Unsupported cmdproof mode: ${mode}`);
  }

  const proof = resolveProof(id, env, cwd);
  const paths = resolvePaths(env);
  const options = { cwd, env, spawnSyncFn, stdout, stderr };
  ensureKeypair(paths, options);

  if (mode === 'check') {
    return runCheck(proof, paths, options);
  }

  return runMode(mode, proof, paths, options);
}

module.exports = {
  buildCmdproofArgs,
  parseCommandToArgv,
  runCmdproof,
  proofLockName,
};
