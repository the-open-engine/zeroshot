'use strict';

const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { it } = require('node:test');
const yaml = require('js-yaml');

function execute(command, args, options) {
  const result = spawnSync(command, args, { encoding: 'utf8', ...options });
  assert.equal(result.status, 0, result.error?.message ?? result.stdout + result.stderr);
  return result.stdout.trim();
}

function sourceFixture(directory) {
  const git = (args, input) => execute('git', args, { cwd: directory, input });
  git(['init', '--quiet']);
  git(['config', 'user.name', 'Docs test']);
  git(['config', 'user.email', 'docs-test@zeroshot.invalid']);
  fs.mkdirSync(path.join(directory, 'scripts'));
  fs.mkdirSync(path.join(directory, 'docs/project'), { recursive: true });
  fs.writeFileSync(path.join(directory, 'scripts/docs_hook.py'), 'release hook');
  fs.writeFileSync(path.join(directory, 'docs/project/versioning.md'), 'release policy');
  fs.writeFileSync(path.join(directory, 'source-marker'), 'release');
  git(['add', '.']);
  git(['commit', '--quiet', '-m', 'release source']);
  const release = git(['rev-parse', 'HEAD']);
  fs.writeFileSync(path.join(directory, 'source-marker'), 'main');
  git(['commit', '--quiet', '-am', 'main source']);
  const main = git(['rev-parse', 'HEAD']);
  git(['update-ref', 'refs/remotes/origin/main', main]);
  git(['checkout', '--quiet', '--detach', release]);
  return { git, release, main };
}

it('migrates published documentation and guards minor updates', () => {
  const result = spawnSync('python3', ['-m', 'unittest', 'discover', '-s', 'tests/docs', '-v'], {
    cwd: path.resolve(__dirname, '../..'),
    encoding: 'utf8',
  });
  assert.equal(result.status, 0, result.error?.message ?? result.stdout + result.stderr);
});

it('bootstraps missing Current from main without inheriting release identity', (t) => {
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot docs bootstrap-'));
  t.after(() => fs.rmSync(temporary, { recursive: true, force: true }));
  const repository = path.join(temporary, 'source');
  const commands = path.join(temporary, 'commands');
  fs.mkdirSync(repository);
  fs.mkdirSync(commands);
  const { git, release, main } = sourceFixture(repository);
  for (const name of ['scripts/docs_hook.py', 'docs/project/versioning.md']) {
    const destination = path.join(repository, '.docs-publisher', name);
    fs.mkdirSync(path.dirname(destination), { recursive: true });
    fs.writeFileSync(destination, 'publication policy');
  }
  fs.writeFileSync(path.join(commands, 'cargo'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  fs.writeFileSync(
    path.join(commands, 'mike'),
    '#!/bin/sh\nset -eu\n' +
      'cmp scripts/docs_hook.py "$GITHUB_WORKSPACE/.docs-publisher/scripts/docs_hook.py"\n' +
      'printf "%s\\n" "$ZEROSHOT_DOCS_VERSION" "$ZEROSHOT_PRODUCT_DOCS_VERSION" ' +
      '"$ZEROSHOT_DOCS_COMMIT" "$(cat source-marker)" > "$CAPTURE"\n',
    { mode: 0o755 }
  );
  const workflow = yaml.load(
    fs.readFileSync(path.resolve(__dirname, '../../.github/workflows/docs.yml'), 'utf8')
  );
  const bootstrap = workflow.jobs.publish.steps.find(
    (step) => step.name === 'Initialize Current before a first release publication'
  );
  const capture = path.join(temporary, 'captured-identity');
  const options = {
    cwd: repository,
    input: bootstrap.run,
    env: {
      ...process.env,
      PATH: `${commands}${path.delimiter}${process.env.PATH}`,
      GITHUB_WORKSPACE: repository,
      RUNNER_TEMP: temporary,
      CAPTURE: capture,
      ZEROSHOT_DOCS_VERSION: 'v10.2',
      ZEROSHOT_PRODUCT_DOCS_VERSION: '10.2.8',
      ZEROSHOT_DOCS_COMMIT: release,
    },
  };
  execute('bash', [], options);
  assert.equal(fs.readFileSync(capture, 'utf8'), `current\n\n${main}\nmain\n`);
  assert.equal(git(['rev-parse', 'HEAD']), release);
  assert.equal(fs.existsSync(path.join(temporary, 'zeroshot-current-bootstrap')), false);

  const manifest = git(['hash-object', '-w', '--stdin'], '{}');
  const current = git(['mktree'], `100644 blob ${manifest}\tmanifest.json\n`);
  const tree = git(['mktree'], `040000 tree ${current}\tcurrent\n`);
  const published = git(['commit-tree', tree, '-m', 'existing Current']);
  git(['update-ref', 'refs/heads/gh-pages', published]);
  fs.unlinkSync(capture);
  execute('bash', [], options);
  assert.equal(fs.existsSync(capture), false, 'an existing Current must not be rebuilt');
});
