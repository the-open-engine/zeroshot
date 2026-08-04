/** Packaging smoke test for the hosted target cutover in the published artifact. */
const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFileSync, execFile } = require('child_process');

const repoRoot = path.join(__dirname, '..');

function execute(command, args, cwd) {
  return execFileSync(command, args, {
    cwd,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  });
}

function packAndInstall() {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-hosted-gate-package-'));
  const output = execute(
    'npm',
    ['pack', '--json', '--ignore-scripts', '--pack-destination', directory],
    repoRoot
  );
  const [{ filename }] = JSON.parse(output);
  const tarball = path.join(directory, filename);
  fs.writeFileSync(path.join(directory, 'package.json'), JSON.stringify({ private: true }));
  execute(
    'npm',
    [
      'install',
      '--ignore-scripts',
      '--omit=optional',
      '--no-package-lock',
      '--no-audit',
      '--no-fund',
      tarball,
    ],
    directory
  );
  return directory;
}

function installedCliPath(directory) {
  return path.join(directory, 'node_modules', '@the-open-engine', 'zeroshot', 'cli', 'index.js');
}

function assertPackageSubpathUnavailable(directory, subpath) {
  try {
    execute(process.execPath, ['-e', `require.resolve(${JSON.stringify(subpath)})`], directory);
    assert.fail(`packed internal hosted subpath resolved: ${subpath}`);
  } catch (error) {
    if (error?.code === 'ERR_ASSERTION') throw error;
    const detail = `${error?.stderr ?? ''}\n${error?.message ?? ''}`;
    assert.match(detail, /MODULE_NOT_FOUND|ERR_PACKAGE_PATH_NOT_EXPORTED|Cannot find module/);
  }
}

function runCli(cliPath, args, settingsFile) {
  return new Promise((resolve) => {
    execFile(
      process.execPath,
      [cliPath, ...args],
      {
        env: {
          ...process.env,
          ZEROSHOT_SETTINGS_FILE: settingsFile,
          NODE_NO_WARNINGS: '1',
        },
        timeout: 15_000,
      },
      (error, stdout, stderr) => {
        resolve({ stdout: stdout ?? '', stderr: stderr ?? '', exitCode: error ? 1 : 0 });
      }
    );
  });
}

describe('packed CLI hosted target cutover', function () {
  this.timeout(180_000);

  let packageDirectory;
  let cliPath;
  let settingsDirectory;
  let settingsFile;

  before(function () {
    packageDirectory = packAndInstall();
    cliPath = installedCliPath(packageDirectory);
    assert.ok(fs.existsSync(cliPath), `packed CLI entrypoint missing at ${cliPath}`);
  });

  after(function () {
    if (packageDirectory) {
      fs.rmSync(packageDirectory, { recursive: true, force: true });
    }
  });

  beforeEach(function () {
    settingsDirectory = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-hosted-gate-settings-'));
    settingsFile = path.join(settingsDirectory, 'settings.json');
  });

  afterEach(function () {
    fs.rmSync(settingsDirectory, { recursive: true, force: true });
  });

  it('does not publish the internal hosted command constructor', function () {
    assertPackageSubpathUnavailable(
      packageDirectory,
      '@the-open-engine/zeroshot/lib/target/register-hosted-commands.js'
    );
    assertPackageSubpathUnavailable(
      packageDirectory,
      '@the-open-engine/zeroshot/src/target/register-hosted-commands.ts'
    );
  });

  it('publishes target management while keeping capsule unpublished', async function () {
    const result = await runCli(cliPath, ['--help'], settingsFile);
    assert.strictEqual(result.exitCode, 0, result.stderr);
    assert.match(result.stdout, /target\s+Manage named remote targets/);
    assert.ok(!/^\s+capsule\b/m.test(result.stdout));

    const runHelp = await runCli(cliPath, ['run', '--help'], settingsFile);
    assert.strictEqual(runHelp.exitCode, 0, runHelp.stderr);
    for (const flag of ['--target', '--size', '--repository', '--submission-key']) {
      assert.ok(runHelp.stdout.includes(flag), `packed run help omitted ${flag}`);
    }
  });

  it('executes packed target commands and rejects unpublished surfaces', async function () {
    const help = await runCli(cliPath, ['target', '--help'], settingsFile);
    assert.strictEqual(help.exitCode, 0, help.stderr);
    assert.match(help.stdout, /status/);
    assert.match(help.stdout, /cancel/);

    const list = await runCli(cliPath, ['target', 'list', '--json'], settingsFile);
    assert.strictEqual(list.exitCode, 0, list.stderr);
    assert.deepStrictEqual(JSON.parse(list.stdout), []);

    for (const args of [['capsule'], ['--all-targets']]) {
      const result = await runCli(cliPath, args, settingsFile);
      assert.notStrictEqual(result.exitCode, 0, `zeroshot ${args.join(' ')} unexpectedly passed`);
      assert.match(result.stderr, /unknown command|unknown option/i);
    }
  });
});
