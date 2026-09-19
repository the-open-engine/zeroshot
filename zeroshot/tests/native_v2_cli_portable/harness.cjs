const fs = require('node:fs');
const readline = require('node:readline');
const { spawn } = require('node:child_process');
const emit = (value) => process.stdout.write(JSON.stringify(value) + '\n');
if (process.argv.includes('app-server')) {
  readline.createInterface({ input: process.stdin }).on('line', (line) => {
    const request = JSON.parse(line);
    if (request.method === 'initialize') emit({ id: request.id, result: {} });
    if (request.method === 'config/read') emit({ id: request.id, result: {
      config: { sandbox_mode: 'workspace-write', approval_policy: 'never', permissions: null,
        active_permissions: null, sandbox_workspace_write: null, projects: null }, origins: {}, layers: []
    } });
    if (request.method === 'configRequirements/read') emit({ id: request.id, result: { requirements: null } });
  });
} else {
  if (process.env.CODEX_API_KEY !== 'fixture-key' || process.env.UNDECLARED_SECRET) process.exit(41);
  const mode = process.env.FIXTURE_MODE;
    if (mode === 'volume') {
      for (let i = 0; i < 256; i++) emit({ type: 'item.completed', item: {
        type: 'command_execution', command: 'fixture', aggregated_output: 'x'.repeat(16384), exit_code: 0
      } });
    }
  let inputBytes = 0;
  process.stdin.on('data', (chunk) => { inputBytes += chunk.length; });
  process.stdin.on('end', () => {
    if (mode === 'volume' && inputBytes < 256 * 1024) process.exit(42);
    fs.writeFileSync('mutation.txt', 'native worker ran');
    emit({ type: 'thread.started', thread_id: 'portable-session' });
    if (mode === 'block') {
      const child = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { stdio: 'ignore' });
      fs.writeFileSync('child.pid', String(child.pid));
      setInterval(() => emit({ type: 'turn.started' }), 50);
      return;
    }
    const correction = mode === 'correction' && !process.argv.includes('resume');
    if (process.argv.includes('resume')) fs.writeFileSync('resumed.txt', 'same session');
    emit({ type: 'item.completed', item: {
      type: 'agent_message', text: correction ? 'invalid-json' : JSON.stringify({ response: null })
    } });
    process.stdout.write(JSON.stringify({ type: 'turn.completed', usage: { input_tokens: 5, output_tokens: 2 } }));
  });
}
