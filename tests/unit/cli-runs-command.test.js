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

  it('waits for detached clusters to register before printing success', function () {
    const cliPath = path.join(__dirname, '..', '..', 'cli', 'index.js');
    const cliCode = fs.readFileSync(cliPath, 'utf8');

    assert(
      cliCode.includes('await waitForClusterRegistration({'),
      'detached run path should wait for cluster registration before reporting success'
    );
    assert(
      cliCode.includes('printDetachedClusterStart(options, clusterId);'),
      'detached run path should still print start info after registration succeeds'
    );
  });

  it('falls back to historical run status when live cluster status fails', function () {
    const cliPath = path.join(__dirname, '..', '..', 'cli', 'index.js');
    const cliCode = fs.readFileSync(cliPath, 'utf8');

    assert(
      cliCode.includes('const historicalRun = findHistoricalRun(id);'),
      'cluster status path should look up historical runs on live status failure'
    );
    assert(
      cliCode.includes("JSON.stringify({ type: 'cluster-history', ...historicalRun }, null, 2)"),
      'cluster status JSON output should fall back to historical run summaries'
    );
  });
});
