import { execFile } from 'child_process';
import { promisify } from 'util';

const execFileAsync = promisify(execFile);

export function isProcessRunning(pid) {
  if (!pid) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

export function killTask(pid) {
  if (!pid) return false;
  try {
    process.kill(pid, 'SIGTERM');
    return true;
  } catch {
    return false;
  }
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function normalizeTerminationOwnership(pid, options) {
  const terminationStrategy = options.terminationStrategy || 'process';
  const processGroupId = Number(options.processGroupId) || null;

  if (terminationStrategy === 'process-group') {
    if (process.platform === 'win32') {
      throw new Error(
        'Process-group termination is unavailable on Windows; use terminationStrategy "process-tree"'
      );
    }
    if (!processGroupId || processGroupId !== pid) {
      throw new Error(
        `Refusing process-group termination for PID ${pid}: owned processGroupId must equal the provider root PID`
      );
    }
  }

  if (terminationStrategy === 'process-tree' && process.platform !== 'win32') {
    throw new Error(
      `Process-tree termination is only supported on Windows; use terminationStrategy "process-group" on ${process.platform}`
    );
  }

  if (!['process', 'process-group', 'process-tree'].includes(terminationStrategy)) {
    throw new Error(`Unsupported termination strategy: ${terminationStrategy}`);
  }

  return { terminationStrategy, processGroupId };
}

function isProcessGroupRunning(processGroupId) {
  try {
    process.kill(-processGroupId, 0);
    return true;
  } catch (error) {
    return error?.code === 'EPERM';
  }
}

export function isOwnedProcessTreeRunning(pid, options = {}) {
  if (!pid) return false;
  const ownership = normalizeTerminationOwnership(pid, options);
  if (ownership.terminationStrategy === 'process-group') {
    return isProcessGroupRunning(ownership.processGroupId);
  }
  return isProcessRunning(pid);
}

async function signalWindowsProcessTree(pid, force) {
  const args = ['/PID', String(pid), '/T'];
  if (force) args.push('/F');
  await execFileAsync('taskkill', args, { windowsHide: true });
}

async function signalOwnedProcessTree(pid, signal, ownership) {
  if (ownership.terminationStrategy === 'process-group') {
    process.kill(-ownership.processGroupId, signal);
    return;
  }
  if (ownership.terminationStrategy === 'process-tree') {
    await signalWindowsProcessTree(pid, signal === 'SIGKILL');
    return;
  }
  process.kill(pid, signal);
}

async function waitForOwnedProcessTreeExit(pid, ownership, timeoutMs, pollMs) {
  const deadline = Date.now() + timeoutMs;
  const options = {
    terminationStrategy: ownership.terminationStrategy,
    processGroupId: ownership.processGroupId,
  };
  while (Date.now() < deadline) {
    if (!isOwnedProcessTreeRunning(pid, options)) return true;
    await sleep(pollMs);
  }
  return !isOwnedProcessTreeRunning(pid, options);
}

function terminationResult(ownership, overrides = {}) {
  const degraded = ownership.terminationStrategy === 'process';
  return {
    terminated: false,
    alreadyDead: false,
    escalated: false,
    signal: null,
    scope: ownership.terminationStrategy,
    degraded,
    degradedReason: degraded
      ? 'Task has no process-tree ownership metadata; only the provider root PID can be terminated'
      : null,
    ...overrides,
  };
}

async function signalAndWait(pid, ownership, signal, timeoutMs, pollMs) {
  const escalated = signal === 'SIGKILL';
  try {
    await signalOwnedProcessTree(pid, signal, ownership);
  } catch (error) {
    if (
      !isOwnedProcessTreeRunning(pid, {
        terminationStrategy: ownership.terminationStrategy,
        processGroupId: ownership.processGroupId,
      })
    ) {
      return terminationResult(ownership, {
        terminated: true,
        alreadyDead: signal === 'SIGTERM',
        escalated,
        signal: escalated ? signal : null,
      });
    }
    return terminationResult(ownership, {
      escalated,
      signal,
      error: error.message,
    });
  }

  const terminated = await waitForOwnedProcessTreeExit(pid, ownership, timeoutMs, pollMs);
  return terminationResult(ownership, {
    terminated,
    escalated,
    signal,
    error:
      terminated || !escalated
        ? null
        : `Owned ${ownership.terminationStrategy} for PID ${pid} survived ${signal}`,
  });
}

/**
 * Terminate an owned provider process tree. Watchers create a dedicated process
 * group on POSIX and persist that ownership boundary; Windows uses taskkill /T.
 * Legacy tasks without ownership metadata fall back to root-only termination
 * and report the degraded scope explicitly.
 */
export async function terminateProcess(pid, options = {}) {
  let ownership;
  try {
    ownership = normalizeTerminationOwnership(pid, options);
  } catch (error) {
    return {
      terminated: false,
      alreadyDead: false,
      escalated: false,
      signal: null,
      error: error.message,
    };
  }

  if (!isOwnedProcessTreeRunning(pid, options)) {
    return terminationResult(ownership, { terminated: true, alreadyDead: true });
  }

  const graceful = await signalAndWait(
    pid,
    ownership,
    'SIGTERM',
    options.graceMs ?? 5000,
    options.pollMs ?? 50
  );
  if (graceful.terminated || graceful.error) return graceful;

  return signalAndWait(
    pid,
    ownership,
    'SIGKILL',
    options.hardKillWaitMs ?? 1000,
    options.pollMs ?? 50
  );
}
