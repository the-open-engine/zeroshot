#!/usr/bin/env node
import { runProviderExecutable } from './contract';
import { spawnProcessRunner } from './process-runner';
import { stringifyJson, unknownToMessage } from './json';

async function readStdin(): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of process.stdin) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(String(chunk)));
  }
  return Buffer.concat(chunks).toString('utf8');
}

async function main(): Promise<void> {
  const input = await readStdin();
  const response = await runProviderExecutable(input, { runner: spawnProcessRunner() });
  process.stdout.write(`${stringifyJson(response.envelope)}\n`);
  process.exitCode = response.exitCode;
}

main().catch((error: unknown) => {
  process.stderr.write(`${unknownToMessage(error)}\n`);
  process.exitCode = 5;
});
