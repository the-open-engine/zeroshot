#!/usr/bin/env node
/**
 * Fake GitHub Copilot CLI for the deterministic Copilot-provider e2e test.
 *
 * Unlike the fake-agent (which shims `claude` via ZEROSHOT_CLAUDE_COMMAND), a generic
 * registry provider is resolved from PATH by its binary name. The e2e harness prepends an
 * isolated bin dir to PATH, so dropping an executable literally named `copilot` there makes
 * the whole stack (CLI parsing -> registry resolution -> preflight availability probe ->
 * spawn -> Copilot JSONL parsing -> ledger -> hooks) run for real, offline, with no API calls.
 *
 * Behaviour:
 *   copilot --version            -> print a version line, exit 0 (availability probe).
 *   copilot --help / -h          -> print usage listing every flag the adapter emits, exit 0
 *                                   (drives detectCliFeatures + the help-or-version probe).
 *   copilot ... -p <prompt> ...  -> a non-interactive run: write COPILOT_FAKE_FILE (default
 *                                   output.txt) into process.cwd() and emit the assumed Copilot
 *                                   `--output-format json` JSONL (assistant text + success result).
 *
 * The prompt is the LAST argv element (adapter emits `-p <prompt>` last). The runtime sets the
 * child cwd to the worktree, so files are written relative to process.cwd() (proves cwd injection).
 */

const fs = require('fs');
const path = require('path');

const USAGE = [
  'GitHub Copilot CLI (fake)',
  '',
  'Usage: copilot [options]',
  '  -p, --prompt <text>        Execute a prompt in non-interactive mode',
  "  --output-format <format>   Output format: 'text' (default) or 'json' (JSONL)",
  '  --model <model>            Set the AI model to use',
  '  --allow-all                Enable all permissions',
  '  --no-ask-user              Disable the ask_user tool',
  '  --add-dir <directory>      Add a directory to the allowed list',
  '  --additional-mcp-config <json>  JSON string or @file MCP config (repeatable)',
].join('\n');

function emit(event) {
  process.stdout.write(`${JSON.stringify(event)}\n`);
}

function promptValue(argv) {
  const flag = argv.lastIndexOf('-p') >= 0 ? argv.lastIndexOf('-p') : argv.lastIndexOf('--prompt');
  if (flag >= 0 && flag + 1 < argv.length) return argv[flag + 1];
  return argv[argv.length - 1] || '';
}

function main() {
  const argv = process.argv.slice(2);

  if (argv.includes('--version')) {
    process.stdout.write('GitHub Copilot CLI 1.0.69 (fake)\n');
    process.exit(0);
  }

  const isRun = argv.includes('-p') || argv.includes('--prompt');
  if (!isRun || argv.includes('--help') || argv.includes('-h')) {
    process.stdout.write(`${USAGE}\n`);
    process.exit(0);
  }

  // Non-interactive run: apply the (fake) model's file change in the worktree cwd, then stream
  // Copilot-shaped JSONL that the copilot adapter parser normalizes into engine OutputEvents.
  const outFile = process.env.COPILOT_FAKE_FILE || 'output.txt';
  const content = process.env.COPILOT_FAKE_CONTENT || 'copilot implemented\n';
  const target = path.resolve(process.cwd(), outFile);
  fs.mkdirSync(path.dirname(target), { recursive: true });
  fs.writeFileSync(target, content);
  process.stderr.write(`fake-copilot: wrote ${target} (prompt="${promptValue(argv)}")\n`);

  // Record the exact argv this run received so the MCP e2e can assert that
  // `--additional-mcp-config <config>` was forwarded through the whole stack.
  const argvLog = path.resolve(process.cwd(), 'copilot-received-argv.json');
  fs.writeFileSync(argvLog, JSON.stringify(argv));

  // Emit the real Copilot `--output-format json` JSONL shape (verified against copilot v1.0.69):
  // dot-namespaced types, payload under `data`, success derived from the terminal `exitCode`.
  emit({ type: 'session.tools_updated', data: { model: 'fake' }, ephemeral: true });
  // A `commentary` message (parsed as thinking) followed by the `final_answer` (parsed as text).
  emit({
    type: 'assistant.message_start',
    data: { messageId: 'm0', phase: 'commentary' },
    ephemeral: true,
  });
  emit({
    type: 'assistant.message',
    data: {
      messageId: 'm0',
      phase: 'commentary',
      content: 'Writing the requested file.',
      outputTokens: 2,
    },
  });
  emit({
    type: 'assistant.message_start',
    data: { messageId: 'm1', phase: 'final_answer' },
    ephemeral: true,
  });
  emit({
    type: 'assistant.message_delta',
    data: { messageId: 'm1', deltaContent: 'Implemented the requested feature.' },
    ephemeral: true,
  });
  emit({
    type: 'assistant.message',
    data: {
      messageId: 'm1',
      phase: 'final_answer',
      content: 'Implemented the requested feature.',
      toolRequests: [],
      outputTokens: 3,
    },
  });
  emit({ type: 'assistant.idle', data: {}, ephemeral: true });
  emit({ type: 'result', sessionId: 'fake', exitCode: 0, usage: { premiumRequests: 0 } });
  process.exit(0);
}

main();
