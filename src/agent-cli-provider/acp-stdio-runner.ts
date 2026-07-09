/* eslint-disable max-lines-per-function */
import { spawn } from 'node:child_process';
import { omitProcessControlEnv, omitUnsafeProviderEnv } from './env-safety';
import type { CommandSpec, ProviderId } from './types';
import type { ProcessResult, ProcessRunnerOptions } from './process-runner';

const DEFAULT_TIMEOUT_KILL_GRACE_MS = 100;

interface JsonRpcRequest {
  readonly jsonrpc: '2.0';
  readonly id: number;
  readonly method: string;
  readonly params: Record<string, unknown>;
}

interface JsonRpcResponse {
  readonly jsonrpc?: string;
  readonly id?: number | string | null;
  readonly result?: unknown;
  readonly error?: unknown;
}

interface PendingRequest {
  resolve(response: JsonRpcResponse): void;
  reject(error: Error): void;
}

function spawnOptions(commandSpec: CommandSpec): {
  readonly shell: false;
  readonly env: NodeJS.ProcessEnv;
  readonly stdio: ['pipe', 'pipe', 'pipe'];
  cwd?: string;
} {
  const options: {
    readonly shell: false;
    readonly env: NodeJS.ProcessEnv;
    readonly stdio: ['pipe', 'pipe', 'pipe'];
    cwd?: string;
  } = {
    shell: false,
    env: { ...omitProcessControlEnv(process.env), ...omitUnsafeProviderEnv(commandSpec.env) },
    stdio: ['pipe', 'pipe', 'pipe'],
  };
  if (commandSpec.cwd !== undefined) options.cwd = commandSpec.cwd;
  return options;
}

function processResult(input: {
  readonly stdout: readonly Buffer[];
  readonly stderr: readonly Buffer[];
  readonly exitCode: number | null;
  readonly signal: string | null;
  readonly startedAt: number;
  readonly timedOut: boolean;
  readonly timeoutMs?: number;
}): ProcessResult {
  return {
    stdout: Buffer.concat(input.stdout).toString('utf8'),
    stderr: Buffer.concat(input.stderr).toString('utf8'),
    exitCode: input.exitCode,
    signal: input.signal,
    durationMs: Date.now() - input.startedAt,
    timedOut: input.timedOut,
    ...(input.timedOut && input.timeoutMs !== undefined ? { timeoutMs: input.timeoutMs } : {}),
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function parseProtocolLine(line: string): Record<string, unknown> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(line);
  } catch (error) {
    const reason = error instanceof Error ? error.message : 'Unknown JSON parse error.';
    throw new Error(`malformed ACP stdout JSON: ${reason}`);
  }
  if (!isRecord(parsed)) throw new Error('malformed ACP stdout JSON: expected a JSON object.');
  return parsed;
}

function createPromptParams(sessionId: string, prompt: string): Record<string, unknown> {
  return {
    sessionId,
    prompt: [
      {
        role: 'user',
        content: [{ type: 'text', text: prompt }],
      },
    ],
  };
}

export function runAcpStdioPrompt(
  provider: ProviderId,
  commandSpec: CommandSpec,
  prompt: string,
  options: ProcessRunnerOptions = {}
): Promise<ProcessResult> {
  return new Promise<ProcessResult>((resolve, reject) => {
    const startedAt = Date.now();
    const stdout: Buffer[] = [];
    const stderr: Buffer[] = [];
    const child = spawn(commandSpec.binary, [...commandSpec.args], spawnOptions(commandSpec));
    const pending = new Map<number, PendingRequest>();
    let stdoutBuffer = '';
    let nextId = 1;
    let timedOut = false;
    let protocolFailure: Error | null = null;
    let sessionId: string | null = null;
    let closeResolved = false;
    let timeout: NodeJS.Timeout | undefined;
    let timeoutKill: NodeJS.Timeout | undefined;

    function cleanupTimers(): void {
      if (timeout !== undefined) clearTimeout(timeout);
      if (timeoutKill !== undefined) clearTimeout(timeoutKill);
    }

    function finalize(exitCode: number | null, signal: string | null): void {
      if (closeResolved) return;
      closeResolved = true;
      cleanupTimers();
      if (protocolFailure) {
        const failureMessage = protocolFailure.message;
        if (!stderr.some((chunk) => chunk.toString('utf8').includes(failureMessage))) {
          stderr.push(Buffer.from(`${failureMessage}\n`, 'utf8'));
        }
      }
      const effectiveExitCode =
        protocolFailure && exitCode === 0 && signal === null ? 1 : exitCode;
      resolve(
        processResult({
          stdout,
          stderr,
          exitCode: effectiveExitCode,
          signal,
          startedAt,
          timedOut,
          ...(options.timeoutMs === undefined ? {} : { timeoutMs: options.timeoutMs }),
        })
      );
    }

    function failClosed(message: string): void {
      if (protocolFailure) return;
      protocolFailure = new Error(`${provider} ACP stdio fail-closed: ${message}`);
      for (const request of pending.values()) {
        request.reject(protocolFailure);
      }
      pending.clear();
      if (sessionId && child.stdin && !child.stdin.destroyed) {
        child.stdin.write(
          `${JSON.stringify({
            jsonrpc: '2.0',
            method: 'session/cancel',
            params: { sessionId },
          })}\n`
        );
      }
      child.stdin?.end();
      child.kill('SIGTERM');
    }

    function handleProtocolMessage(message: Record<string, unknown>): void {
      const responseId = typeof message.id === 'number' ? message.id : null;
      if (responseId !== null && (message.result !== undefined || message.error !== undefined)) {
        const request = pending.get(responseId);
        if (request) {
          pending.delete(responseId);
          request.resolve(message);
        }
        return;
      }

      const method = typeof message.method === 'string' ? message.method : null;
      if (!method) return;
      if (method === 'session/update') return;
      if (method.startsWith('_')) return;
      if (method === 'session/request_permission') {
        failClosed('unsupported session/request_permission callback.');
        return;
      }
      if (method.startsWith('fs/')) {
        failClosed(`unsupported ${method} callback.`);
        return;
      }
      if (method.startsWith('terminal/')) {
        failClosed(`unsupported ${method} callback.`);
        return;
      }
      if (method.startsWith('session/')) {
        failClosed(`unsupported ${method} session-control callback.`);
      }
    }

    function consumeStdoutLine(line: string): void {
      try {
        handleProtocolMessage(parseProtocolLine(line));
      } catch (error) {
        failClosed(error instanceof Error ? error.message : 'malformed ACP stdout JSON.');
      }
    }

    function flushStdoutRemainder(): void {
      const line = stdoutBuffer.trim();
      stdoutBuffer = '';
      if (line) consumeStdoutLine(line);
    }

    function flushStdout(data: Buffer): void {
      stdout.push(data);
      stdoutBuffer += data.toString('utf8');
      let newline = stdoutBuffer.indexOf('\n');
      while (newline !== -1) {
        const line = stdoutBuffer.slice(0, newline).trim();
        stdoutBuffer = stdoutBuffer.slice(newline + 1);
        if (line) consumeStdoutLine(line);
        newline = stdoutBuffer.indexOf('\n');
      }
    }

    function sendRequest(method: string, params: Record<string, unknown>): Promise<JsonRpcResponse> {
      const id = nextId;
      nextId += 1;
      const request: JsonRpcRequest = {
        jsonrpc: '2.0',
        id,
        method,
        params,
      };
      return new Promise<JsonRpcResponse>((requestResolve, requestReject) => {
        pending.set(id, { resolve: requestResolve, reject: requestReject });
        child.stdin?.write(`${JSON.stringify(request)}\n`);
      });
    }

    async function driveProtocol(): Promise<void> {
      const initialize = await sendRequest('initialize', {
        protocolVersion: 1,
        clientInfo: {
          name: '@the-open-engine/zeroshot',
          version: '1',
        },
        clientCapabilities: {
          fs: false,
          terminal: false,
          promptCapabilities: {
            image: false,
          },
        },
      });
      if (initialize.error !== undefined) {
        child.stdin?.end();
        return;
      }

      const session = await sendRequest('session/new', {
        cwd: commandSpec.cwd ?? process.cwd(),
        mcpServers: [],
      });
      if (session.error !== undefined) {
        child.stdin?.end();
        return;
      }
      const sessionResult = isRecord(session.result) ? session.result : {};
      const nextSessionId = typeof sessionResult.sessionId === 'string' ? sessionResult.sessionId : null;
      if (!nextSessionId) {
        failClosed('session/new response omitted sessionId.');
        return;
      }
      sessionId = nextSessionId;

      await sendRequest('session/prompt', createPromptParams(sessionId, prompt));
      child.stdin?.end();
    }

    if (options.timeoutMs !== undefined) {
      timeout = setTimeout(() => {
        timedOut = true;
        if (sessionId && child.stdin && !child.stdin.destroyed) {
          child.stdin.write(
            `${JSON.stringify({
              jsonrpc: '2.0',
              method: 'session/cancel',
              params: { sessionId },
            })}\n`
          );
        }
        child.kill('SIGTERM');
        timeoutKill = setTimeout(() => {
          if (!closeResolved) child.kill('SIGKILL');
        }, options.timeoutKillGraceMs ?? DEFAULT_TIMEOUT_KILL_GRACE_MS);
      }, options.timeoutMs);
    }

    child.stdout?.on('data', flushStdout);
    child.stdout?.once('end', flushStdoutRemainder);
    child.stderr?.on('data', (data: Buffer) => stderr.push(data));
    child.once('error', (error) => {
      cleanupTimers();
      reject(error);
    });
    child.once('close', (exitCode, signal) => {
      finalize(exitCode, signal);
    });

    driveProtocol().catch((error: unknown) => {
      failClosed(error instanceof Error ? error.message : 'ACP protocol failed.');
    });
  });
}
