#!/usr/bin/env node

const fs = require('fs');

if (process.argv.includes('--version')) {
  console.log('codex-cli 99.0.0');
  process.exit(0);
}
if (process.argv.includes('--help')) {
  console.log(
    'codex exec --json --output-schema -m -C --config --skip-git-repo-check ' +
      '--dangerously-bypass-approvals-and-sandbox'
  );
  process.exit(0);
}

const stateFile = process.env.FAKE_CODEX_COUNT;
const count = Number(fs.existsSync(stateFile) ? fs.readFileSync(stateFile, 'utf8') : '0') + 1;
fs.writeFileSync(stateFile, String(count));
console.log(JSON.stringify({ type: 'turn.started' }));

const actions = JSON.parse(process.env.FAKE_CODEX_ACTIONS || '["success"]');
const action = actions[count - 1] || actions.at(-1);
if (action === 'hang') {
  setInterval(() => {}, 1000);
} else if (action === 'exit') {
  console.error('simulated provider process death');
  process.exit(17);
} else {
  console.log(
    JSON.stringify({
      type: 'item.completed',
      item: { type: 'agent_message', text: '{"done":true}' },
    })
  );
  console.log(
    JSON.stringify({
      type: 'turn.completed',
      usage: { input_tokens: 1, output_tokens: 1 },
    })
  );
}
