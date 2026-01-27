const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const buildOutput = path.join(__dirname, '..', '..', 'lib', 'tui', 'commands', 'parser.js');

function ensureTuiBuild() {
  if (!fs.existsSync(buildOutput)) {
    execSync('npm run build:tui', { stdio: 'inherit' });
  }
}

ensureTuiBuild();

const { parseInput } = require('../../lib/tui/commands/parser');

describe('TUI command parser', function () {
  it('parses slash commands', function () {
    const parsed = parseInput('/help');
    assert.strictEqual(parsed.type, 'command');
    assert.strictEqual(parsed.name, 'help');
    assert.deepStrictEqual(parsed.args, []);
  });

  it('parses command arguments', function () {
    const parsed = parseInput('/issue 123');
    assert.strictEqual(parsed.type, 'command');
    assert.strictEqual(parsed.name, 'issue');
    assert.deepStrictEqual(parsed.args, ['123']);
  });

  it('parses plain text input', function () {
    const parsed = parseInput('hello world');
    assert.strictEqual(parsed.type, 'text');
    assert.strictEqual(parsed.text, 'hello world');
  });
});
