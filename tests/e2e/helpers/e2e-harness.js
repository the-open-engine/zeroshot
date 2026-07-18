/**
 * Harness for deterministic end-to-end tests.
 *
 * Drives the real `zeroshot` binary (cli/index.js) as a subprocess, with
 * ZEROSHOT_CLAUDE_COMMAND pointed at tests/fixtures/fake-agent so every layer
 * above the model (CLI parsing, orchestrator, message bus, ledger, trigger
 * engine, agent spawning, stream-json parsing, hooks, worktree isolation)
 * runs for real, offline, with no API calls.
 *
 * Isolation note: most storage paths in this codebase are derived from
 * `os.homedir()` directly (orchestrator storageDir, worktree paths, clusters.json),
 * NOT from a ZEROSHOT_HOME env var. Node's os.homedir() reads the HOME env var on
 * POSIX, so isolation here is achieved by overriding HOME for the child process,
 * not by inventing a ZEROSHOT_HOME-only scheme.
 *
 * PATH shim note: agent execution self-spawns via `which zeroshot`
 * (src/agent/agent-task-executor.js:_resolveZeroshotPath), i.e. it does NOT
 * reuse the exact script path used to launch the top-level process. A temp
 * directory containing a `zeroshot` shim script is prepended to PATH so the
 * self-spawned subprocess resolves back to this checkout's cli/index.js,
 * without requiring `npm link` as a precondition.
 */

const fs = require('fs');
const path = require('path');
const os = require('os');
const { execFileSync, spawn, spawnSync } = require('child_process');

const REPO_ROOT = path.resolve(__dirname, '..', '..', '..');
const CLI_ENTRY = path.join(REPO_ROOT, 'cli', 'index.js');
const FAKE_AGENT_PATH = path.join(REPO_ROOT, 'tests', 'fixtures', 'fake-agent', 'index.js');
const FIXTURES_DIR = path.join(REPO_ROOT, 'tests', 'fixtures', 'fake-agent-scenarios');

function scenarioPath(name) {
  return path.join(FIXTURES_DIR, name.endsWith('.json') ? name : `${name}.json`);
}

function runGit(args, cwd) {
  const result = spawnSync('git', args, { cwd, encoding: 'utf8', stdio: 'pipe' });
  if (result.status !== 0) {
    throw new Error(
      `git ${args.join(' ')} failed in ${cwd}: ${result.stderr || result.error?.message}`
    );
  }
  return result.stdout;
}

function writeZeroshotShim(binDir) {
  const shimPath = path.join(binDir, 'zeroshot');
  fs.writeFileSync(shimPath, `#!/bin/sh\nexec node "${CLI_ENTRY}" "$@"\n`, { mode: 0o755 });
  fs.chmodSync(shimPath, 0o755);
  return shimPath;
}

/**
 * Sets up an isolated temp git repo (with a local bare "origin") plus an
 * isolated fake HOME so `zeroshot` state (clusters.json, worktrees, settings)
 * never touches the real user environment.
 */
function setupE2ERepo() {
  const repoDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-e2e-repo-'));
  const homeDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-e2e-home-'));
  const binDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-e2e-bin-'));
  const originDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-e2e-origin-'));

  runGit(['init', '--bare'], originDir);

  runGit(['init'], repoDir);
  runGit(['config', 'user.email', 'e2e@example.com'], repoDir);
  runGit(['config', 'user.name', 'E2E Test'], repoDir);
  runGit(['remote', 'add', 'origin', originDir], repoDir);
  fs.writeFileSync(path.join(repoDir, 'README.md'), '# e2e test repo\n');
  runGit(['add', 'README.md'], repoDir);
  runGit(['commit', '-m', 'Initial commit'], repoDir);
  runGit(['push', 'origin', 'HEAD'], repoDir);

  writeZeroshotShim(binDir);
  fs.mkdirSync(path.join(homeDir, '.zeroshot'), { recursive: true });
  // Prevent the CLI's npm-registry update check (network call + interactive
  // prompt) from firing during offline, deterministic test runs.
  fs.writeFileSync(
    path.join(homeDir, '.zeroshot', 'settings.json'),
    JSON.stringify({ autoCheckUpdates: false }, null, 2)
  );

  return { repoDir, homeDir, binDir, originDir };
}

function cleanupE2ERepo(env) {
  if (!env) return;
  for (const dir of [env.repoDir, env.homeDir, env.binDir, env.originDir]) {
    if (dir && fs.existsSync(dir)) {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  }
}

/**
 * Strips ambient ZEROSHOT_ and CMDPROOF_ prefixed env vars from the base env
 * before building a child env. Without this, running the e2e suite from inside a
 * zeroshot-managed session (e.g. a zeroshot worker agent) leaks that
 * session's own ZEROSHOT_RUN_OPTIONS/ZEROSHOT_PR/ZEROSHOT_CLUSTER_ID etc.
 * into the spawned test subprocess, silently changing its behavior
 * (observed: an inherited ZEROSHOT_RUN_OPTIONS with pr:true injected a
 * git-pusher agent and strict schema into an unrelated custom --config).
 */
function baseEnv() {
  const result = {};
  for (const [key, value] of Object.entries(process.env)) {
    if (key.startsWith('ZEROSHOT_') || key.startsWith('CMDPROOF_')) continue;
    result[key] = value;
  }
  return result;
}

function buildEnv(env, envOverrides = {}) {
  return {
    ...baseEnv(),
    HOME: env.homeDir,
    ZEROSHOT_HOME: env.homeDir,
    ZEROSHOT_SETTINGS_FILE: path.join(env.homeDir, '.zeroshot', 'settings.json'),
    ZEROSHOT_CLAUDE_COMMAND: `node ${FAKE_AGENT_PATH}`,
    PATH: `${env.binDir}${path.delimiter}${process.env.PATH}`,
    ...envOverrides,
  };
}

/**
 * Runs `zeroshot <args>` as a real subprocess against the isolated repo/home.
 * Returns { status, stdout, stderr } instead of throwing, so tests can assert
 * on non-zero exit codes (e.g. the failing-agent scenario).
 */
function runZeroshot(env, args, envOverrides = {}) {
  const result = spawnSync('node', [CLI_ENTRY, ...args], {
    cwd: env.repoDir,
    env: buildEnv(env, envOverrides),
    encoding: 'utf8',
    timeout: envOverrides.timeout || 90000,
  });
  return {
    status: result.status,
    stdout: result.stdout || '',
    stderr: result.stderr || '',
    error: result.error || null,
  };
}

function runZeroshotUntilNaturalExit(env, args, envOverrides, completionMarker) {
  return new Promise((resolve, reject) => {
    const child = spawn('node', [CLI_ENTRY, ...args], {
      cwd: env.repoDir,
      env: buildEnv(env, envOverrides),
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let stdout = '';
    let stderr = '';
    let completionAt = null;
    let exitTimer = null;
    const hardTimer = setTimeout(() => {
      child.kill('SIGKILL');
      reject(new Error(`CLI did not exit within ${envOverrides.timeout}ms\n${stdout}\n${stderr}`));
    }, envOverrides.timeout);
    child.stdout.on('data', (chunk) => {
      stdout += chunk;
      if (completionAt === null && stdout.includes(completionMarker)) {
        completionAt = Date.now();
        exitTimer = setTimeout(() => {
          child.kill('SIGKILL');
          reject(new Error(`CLI stayed alive after ${completionMarker}\n${stdout}\n${stderr}`));
        }, 2000);
      }
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk;
    });
    child.on('error', reject);
    child.on('close', (status) => {
      clearTimeout(hardTimer);
      clearTimeout(exitTimer);
      resolve({
        status,
        stdout,
        stderr,
        completionToExitMs: completionAt === null ? null : Date.now() - completionAt,
      });
    });
  });
}

function clustersFilePath(env) {
  return path.join(env.homeDir, '.zeroshot', 'clusters.json');
}

function readClusters(env) {
  const file = clustersFilePath(env);
  if (!fs.existsSync(file)) return {};
  return JSON.parse(fs.readFileSync(file, 'utf8'));
}

function readCluster(env, clusterId) {
  return readClusters(env)[clusterId] || null;
}

/**
 * clusters.json does not persist the worktree path (src/orchestrator.js
 * _saveClusters only persists `isolation`, the Docker-mode counterpart, not
 * `worktree`). The path is deterministic (src/isolation-manager.js
 * createWorktreeIsolation): <homeDir>/.zeroshot/worktrees/<clusterId>.
 */
function worktreePath(env, clusterId) {
  return path.join(env.homeDir, '.zeroshot', 'worktrees', clusterId);
}

async function waitForClusterState(env, clusterId, targetStates, timeoutMs = 30000) {
  const states = Array.isArray(targetStates) ? targetStates : [targetStates];
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    const cluster = readCluster(env, clusterId);
    if (cluster && states.includes(cluster.state)) {
      return cluster;
    }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(
    `Cluster ${clusterId} did not reach state [${states.join(', ')}] within ${timeoutMs}ms. ` +
      `Last known: ${JSON.stringify(readCluster(env, clusterId))}`
  );
}

function clusterDbPath(env, clusterId) {
  return path.join(env.homeDir, '.zeroshot', `${clusterId}.db`);
}

/**
 * Mirrors src/ledger.js's row deserialization: content_text/content_data/metadata
 * are stored as raw TEXT/JSON columns, not reconstructed by a bare SELECT *.
 */
function rowToMessage(row) {
  const message = { ...row, content: {} };
  if (row.content_text) message.content.text = row.content_text;
  if (row.content_data) message.content.data = JSON.parse(row.content_data);
  if (row.metadata) message.metadata = JSON.parse(row.metadata);
  return message;
}

function readLedgerMessages(env, clusterId, topic) {
  const Database = require('better-sqlite3');
  const dbPath = clusterDbPath(env, clusterId);
  if (!fs.existsSync(dbPath)) return [];
  const db = new Database(dbPath, { readonly: true, timeout: 5000 });
  try {
    const rows = db
      .prepare('SELECT * FROM messages WHERE cluster_id = ? AND topic = ? ORDER BY id')
      .all(clusterId, topic);
    return rows.map(rowToMessage);
  } finally {
    db.close();
  }
}

function gitStatusPorcelain(repoDir) {
  return runGit(['status', '--porcelain'], repoDir).trim();
}

function gitWorktreeList(repoDir) {
  return runGit(['worktree', 'list'], repoDir);
}

module.exports = {
  REPO_ROOT,
  CLI_ENTRY,
  FAKE_AGENT_PATH,
  scenarioPath,
  setupE2ERepo,
  cleanupE2ERepo,
  runZeroshot,
  runZeroshotUntilNaturalExit,
  buildEnv,
  clustersFilePath,
  readClusters,
  readCluster,
  worktreePath,
  waitForClusterState,
  clusterDbPath,
  readLedgerMessages,
  gitStatusPorcelain,
  gitWorktreeList,
  execFileSync,
};
