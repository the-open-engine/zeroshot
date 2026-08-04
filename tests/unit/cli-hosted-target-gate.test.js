/** Production CLI coverage for the hosted target cutover. */

const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFile } = require('child_process');

const CLI_PATH = path.resolve(__dirname, '..', '..', 'cli', 'index.js');
const { program: productionProgram } = require('commander');
require('../../cli/index.js');

function collectCommandTree(command, pathParts = []) {
  const currentPath = [...pathParts, command.name()].filter(Boolean);
  return [
    { command, path: currentPath.join(' ') || '<root>' },
    ...command.commands.flatMap((child) => collectCommandTree(child, currentPath)),
  ];
}

const productionCommands = collectCommandTree(productionProgram);
let tmpDir;
let settingsFile;

beforeEach(function () {
  tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-hosted-gate-'));
  settingsFile = path.join(tmpDir, 'settings.json');
});

afterEach(function () {
  fs.rmSync(tmpDir, { recursive: true, force: true });
});

function cli(args) {
  return new Promise((resolve) => {
    execFile(
      process.execPath,
      [CLI_PATH, ...args],
      {
        env: {
          ...process.env,
          ZEROSHOT_SETTINGS_FILE: settingsFile,
          NODE_NO_WARNINGS: '1',
        },
        timeout: 10_000,
      },
      (error, stdout, stderr) => {
        resolve({ stdout: stdout ?? '', stderr: stderr ?? '', exitCode: error ? 1 : 0 });
      }
    );
  });
}

describe('CLI hosted target cutover', function () {
  this.timeout(20_000);

  describe('production parser construction', function () {
    it('publishes the target lifecycle and hosted run options', function () {
      const target = productionProgram.commands.find((command) => command.name() === 'target');
      const run = productionProgram.commands.find((command) => command.name() === 'run');
      assert.ok(target);
      assert.ok(run);
      assert.deepStrictEqual(target.commands.map((command) => command.name()).sort(), [
        'add',
        'cancel',
        'list',
        'login',
        'remove',
        'status',
      ]);
      for (const flag of ['--target', '--size', '--repository', '--submission-key']) {
        assert.ok(
          run.options.some((option) => option.long === flag),
          `run omitted ${flag}`
        );
      }
      assert.ok(!productionCommands.some(({ command }) => command.name() === 'capsule'));
      assert.ok(
        !productionCommands.some(({ command }) =>
          command.options.some((option) => option.long === '--all-targets')
        )
      );
    });
  });

  describe('process boundary', function () {
    it('renders target help from the production entrypoint', async function () {
      const result = await cli(['target', '--help']);
      assert.strictEqual(result.exitCode, 0, result.stderr);
      assert.match(result.stdout, /Manage named remote targets/);
      assert.match(result.stdout, /status/);
      assert.match(result.stdout, /cancel/);
    });

    it('lists zero configured targets as JSON', async function () {
      const result = await cli(['target', 'list', '--json']);
      assert.strictEqual(result.exitCode, 0, result.stderr);
      assert.deepStrictEqual(JSON.parse(result.stdout), []);
    });

    it('keeps the unpublished capsule command unavailable', async function () {
      const result = await cli(['capsule']);
      assert.notStrictEqual(result.exitCode, 0);
      assert.match(result.stderr, /unknown command/i);
    });

    it('rejects "--all-targets"', async function () {
      const result = await cli(['--all-targets']);
      assert.notStrictEqual(result.exitCode, 0);
      assert.match(result.stderr, /unknown option/i);
    });
  });
});
