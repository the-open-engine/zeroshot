const { spawn } = require('child_process');
const { existsSync, mkdirSync, readFileSync } = require('fs');
const { dirname, resolve } = require('path');
const { pathToFileURL } = require('url');

const watcherName = process.argv[2];
const repoRoot = resolve(__dirname, '../..');
const watcherPath = resolve(repoRoot, 'task-lib', watcherName);
const providerPath = resolve(__dirname, 'sigterm-root-with-child.js');
const logFile = resolve(process.env.HOME, '.zeroshot', `${watcherName}.log`);
const taskId = watcherName === 'attachable-watcher.js' ? 'runtime-a' : 'runtime-w';

const sleep = (ms) => new Promise((resolvePromise) => setTimeout(resolvePromise, ms));

async function waitFor(predicate, timeoutMs = 20000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const result = predicate();
    if (result) return result;
    await sleep(20);
  }
  throw new Error(`Timed out waiting for ${watcherName} runtime state`);
}

function isRunning(pid) {
  if (!pid) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

function forceKill(pid) {
  if (!pid) return;
  try {
    process.kill(pid, 'SIGKILL');
  } catch {
    // Already stopped.
  }
}

async function main() {
  const storeUrl = pathToFileURL(resolve(repoRoot, 'task-lib/store.js')).href;
  const killUrl = pathToFileURL(resolve(repoRoot, 'task-lib/commands/kill.js')).href;
  const { addTask, getTask } = await import(storeUrl);
  const { killTaskCommand } = await import(killUrl);

  mkdirSync(dirname(logFile), { recursive: true });
  addTask({
    id: taskId,
    prompt: 'runtime ownership proof',
    fullPrompt: 'runtime ownership proof',
    cwd: repoRoot,
    status: 'running',
    pid: null,
    logFile,
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
    provider: 'codex',
    attachable: watcherName === 'attachable-watcher.js',
  });

  const unrelated = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], {
    detached: process.platform !== 'win32',
    stdio: 'ignore',
  });
  const config = {
    provider: 'codex',
    outputFormat: 'stream-json',
    commandSpec: {
      binary: process.execPath,
      args: [providerPath],
      env: {},
      cleanup: [],
    },
  };
  const watcher = spawn(
    process.execPath,
    [watcherPath, taskId, repoRoot, logFile, '[]', JSON.stringify(config)],
    {
      env: process.env,
      stdio: ['ignore', 'pipe', 'pipe'],
    }
  );
  let watcherOutput = '';
  watcher.stdout.on('data', (chunk) => {
    watcherOutput += chunk;
  });
  watcher.stderr.on('data', (chunk) => {
    watcherOutput += chunk;
  });

  let providerPid;
  let descendantPid;
  try {
    const persisted = await waitFor(() => {
      const task = getTask(taskId);
      if (watcher.exitCode !== null && !task?.pid) {
        const logOutput = existsSync(logFile) ? readFileSync(logFile, 'utf8') : '';
        throw new Error(
          `${watcherName} exited ${watcher.exitCode} before persisting ownership: ${watcherOutput}${logOutput}`
        );
      }
      return task?.pid && task.processGroupId && task.terminationStrategy ? task : null;
    });
    providerPid = persisted.pid;
    assertOwnedMetadata(persisted);
    descendantPid = await waitFor(() => readDescendantPid(logFile));

    await killTaskCommand(taskId, {
      graceMs: 100,
      hardKillWaitMs: 500,
      pollMs: 10,
    });
    await waitFor(() => !isRunning(providerPid) && !isRunning(descendantPid));

    const terminal = getTask(taskId);
    const result = {
      watcherName,
      providerPid,
      descendantPid,
      unrelatedAlive: isRunning(unrelated.pid),
      providerAlive: isRunning(providerPid),
      descendantAlive: isRunning(descendantPid),
      persistedStrategy: persisted.terminationStrategy,
      persistedGroupId: persisted.processGroupId,
      terminalGroupId: terminal.processGroupId,
    };
    process.stdout.write(`RESULT:${JSON.stringify(result)}\n`);
  } finally {
    forceKill(watcher.pid);
    forceKill(providerPid);
    forceKill(descendantPid);
    forceKill(unrelated.pid);
  }
}

function assertOwnedMetadata(task) {
  if (process.platform === 'win32') {
    if (task.processGroupId !== null || task.terminationStrategy !== 'process-tree') {
      throw new Error(`Invalid Windows ownership metadata: ${JSON.stringify(task)}`);
    }
    return;
  }
  if (task.processGroupId !== task.pid || task.terminationStrategy !== 'process-group') {
    throw new Error(`Invalid POSIX ownership metadata: ${JSON.stringify(task)}`);
  }
}

function readDescendantPid(path) {
  if (!existsSync(path)) return null;
  const match = readFileSync(path, 'utf8').match(/\](\d+)\r?\n/);
  return match ? Number(match[1]) : null;
}

main().catch((error) => {
  process.stderr.write(`${error.stack || error.message}\n`);
  process.exitCode = 1;
});
