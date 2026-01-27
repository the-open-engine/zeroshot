import { format } from "node:util";

export type CliCompatResult = {
  output: string;
  exitCode: number;
};

class CliCompatExit extends Error {
  code: number;

  constructor(code: number) {
    super("CLI_COMPAT_EXIT");
    this.code = code;
  }
}

function stripAnsi(value: string): string {
  // eslint-disable-next-line no-control-regex
  return value.replace(/\x1b\[[0-9;]*[a-zA-Z]/g, "");
}

function condenseOutput(value: string): string {
  const cleaned = stripAnsi(value).replace(/\r/g, "");
  const lines = cleaned
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
  return lines.join(" | ");
}

function normalizeChunk(chunk: unknown): string {
  if (typeof chunk === "string") return chunk;
  if (Buffer.isBuffer(chunk)) return chunk.toString("utf-8");
  return String(chunk);
}

async function runWithOutput(
  runner: () => void | Promise<void>
): Promise<CliCompatResult> {
  const stdout: string[] = [];
  const stderr: string[] = [];
  let exitCode = 0;

  const originalConsoleLog = console.log;
  const originalConsoleError = console.error;
  const originalConsoleWarn = console.warn;
  const originalStdoutWrite = process.stdout.write.bind(process.stdout);
  const originalStderrWrite = process.stderr.write.bind(process.stderr);
  const originalExit = process.exit;

  console.log = (...args: unknown[]) => {
    stdout.push(format(...args));
  };
  console.error = (...args: unknown[]) => {
    stderr.push(format(...args));
  };
  console.warn = (...args: unknown[]) => {
    stderr.push(format(...args));
  };

  process.stdout.write = ((chunk: unknown, encoding?: unknown, callback?: unknown) => {
    stdout.push(normalizeChunk(chunk));
    if (typeof encoding === "function") {
      encoding();
    } else if (typeof callback === "function") {
      callback();
    }
    return true;
  }) as typeof process.stdout.write;

  process.stderr.write = ((chunk: unknown, encoding?: unknown, callback?: unknown) => {
    stderr.push(normalizeChunk(chunk));
    if (typeof encoding === "function") {
      encoding();
    } else if (typeof callback === "function") {
      callback();
    }
    return true;
  }) as typeof process.stderr.write;

  process.exit = ((code?: number) => {
    exitCode = typeof code === "number" ? code : 0;
    throw new CliCompatExit(exitCode);
  }) as typeof process.exit;

  try {
    await runner();
  } catch (error) {
    if (!(error instanceof CliCompatExit)) {
      throw error;
    }
  } finally {
    console.log = originalConsoleLog;
    console.error = originalConsoleError;
    console.warn = originalConsoleWarn;
    process.stdout.write = originalStdoutWrite;
    process.stderr.write = originalStderrWrite;
    process.exit = originalExit;
  }

  const combined = [...stdout, ...stderr].join("\n");
  return {
    output: condenseOutput(combined),
    exitCode,
  };
}

export async function runListTasks(options: {
  status?: string;
  limit?: number;
  verbose?: boolean;
}): Promise<CliCompatResult> {
  const { listTasks } = await import("../../../task-lib/commands/list.js");
  return runWithOutput(() => listTasks(options));
}

export async function runShowStatus(taskId: string): Promise<CliCompatResult> {
  const { showStatus } = await import("../../../task-lib/commands/status.js");
  return runWithOutput(() => showStatus(taskId));
}
