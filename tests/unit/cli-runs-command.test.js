const assert = require('assert');
const fs = require('fs');
const path = require('path');

describe('CLI runs/status commands', function () {
  it('registers a runs command with timeline filters', function () {
    const cliPath = path.join(__dirname, '..', '..', 'cli', 'index.js');
    const cliCode = fs.readFileSync(cliPath, 'utf8');

    assert(cliCode.includes(".command('runs')"), 'runs command should be registered');
    assert(cliCode.includes(".option('--today'"), 'runs command should expose --today');
    assert(cliCode.includes(".option('--since <when>'"), 'runs command should expose --since');
    assert(cliCode.includes(".option('--running'"), 'runs command should expose --running');
  });

  it('allows zeroshot status with no id to show active runs', function () {
    const cliPath = path.join(__dirname, '..', '..', 'cli', 'index.js');
    const cliCode = fs.readFileSync(cliPath, 'utf8');

    assert(
      cliCode.includes(".command('status [id]')"),
      'status command should accept an optional id'
    );
    assert(
      cliCode.includes('printRunTable(activeRuns'),
      'status without an id should print active runs'
    );
  });
});
