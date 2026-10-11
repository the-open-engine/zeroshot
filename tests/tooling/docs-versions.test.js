'use strict';

const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { it } = require('node:test');
const yaml = require('js-yaml');

// Hooks export repository-local Git settings; fixtures must own their Git state.
const fixtureEnvironment = Object.fromEntries(
  Object.entries(process.env).filter(([name]) => !name.startsWith('GIT_'))
);

function execute(command, args, options) {
  const result = spawnSync(command, args, {
    encoding: 'utf8',
    env: fixtureEnvironment,
    ...options,
  });
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
  git(['add', 'scripts/docs_hook.py', 'docs/project/versioning.md', 'source-marker']);
  git(['commit', '--quiet', '-m', 'release source']);
  const release = git(['rev-parse', 'HEAD']);
  fs.writeFileSync(path.join(directory, 'source-marker'), 'main');
  git(['add', 'source-marker']);
  git(['commit', '--quiet', '-m', 'main source']);
  const main = git(['rev-parse', 'HEAD']);
  git(['update-ref', 'refs/remotes/origin/main', main]);
  git(['checkout', '--quiet', '--detach', release]);
  return { git, release, main };
}

function publisherFixture(t, name) {
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), `zeroshot docs ${name}-`));
  t.after(() => fs.rmSync(temporary, { recursive: true, force: true }));
  const repository = path.join(temporary, 'source');
  const commands = path.join(temporary, 'commands');
  fs.mkdirSync(repository);
  fs.mkdirSync(commands);
  return { temporary, repository, commands, ...sourceFixture(repository) };
}

function publishStep(name) {
  const workflow = yaml.load(
    fs.readFileSync(path.resolve(__dirname, '../../.github/workflows/docs.yml'), 'utf8')
  );
  return workflow.jobs.publish.steps.find((step) => step.name === name);
}

it('migrates published documentation and guards minor updates', () => {
  const result = spawnSync('python3', ['-m', 'unittest', 'discover', '-s', 'tests/docs', '-v'], {
    cwd: path.resolve(__dirname, '../..'),
    encoding: 'utf8',
    env: fixtureEnvironment,
  });
  assert.equal(result.status, 0, result.error?.message ?? result.stdout + result.stderr);
});

it('bootstraps missing Current from main without inheriting release identity', (t) => {
  const { temporary, repository, commands, git, release, main } = publisherFixture(t, 'bootstrap');
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
  const bootstrap = publishStep('Initialize Current before a first release publication');
  const capture = path.join(temporary, 'captured-identity');
  const options = {
    cwd: repository,
    input: bootstrap.run,
    env: {
      ...fixtureEnvironment,
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

it('rechecks minor canonicals against the Current tree it just published', (t) => {
  const { temporary, repository, commands, git } = publisherFixture(t, 'canonical');
  const pages = path.join(temporary, 'pages');
  const publisher = path.join(repository, '.docs-publisher/scripts/docs_versions.py');
  fs.mkdirSync(path.dirname(publisher), { recursive: true });
  fs.copyFileSync(path.resolve(__dirname, '../../scripts/docs_versions.py'), publisher);

  const page = (version, route, origin) => {
    const destination = path.join(pages, version, route, 'index.html');
    fs.mkdirSync(path.dirname(destination), { recursive: true });
    fs.writeFileSync(
      destination,
      `<html><head><link rel="canonical" href="${origin}${version}/${route}"></head></html>`
    );
  };
  for (const route of ['', 'guides/acp/']) {
    page('current', route, 'https://zeroshot.sh/docs/');
    page('v10.2', route, 'https://zeroshot.sh/docs/');
  }
  fs.writeFileSync(
    path.join(pages, 'versions.json'),
    JSON.stringify([
      { version: 'current', title: 'Current', aliases: ['dev'] },
      { version: 'v10.2', title: 'v10.2', aliases: [] },
    ])
  );
  const index = path.join(temporary, 'pages-index');
  const pagesGit = (args) =>
    execute('git', ['--work-tree', pages, ...args], {
      cwd: repository,
      env: { ...fixtureEnvironment, GIT_INDEX_FILE: index },
    });
  pagesGit(['add', '--', 'versions.json', 'current', 'v10.2']);
  const tree = pagesGit(['write-tree']);
  git(['update-ref', 'refs/heads/gh-pages', git(['commit-tree', tree, '-m', 'published'])]);

  // The fake publication drops a Current page that v10.2 still has.
  fs.writeFileSync(
    path.join(commands, 'mike'),
    '#!/bin/sh\nset -eu\n[ "$1" = deploy ] || exit 0\n' +
      'tree="$RUNNER_TEMP/fake-mike"\n' +
      'git worktree add --quiet "$tree" gh-pages\n' +
      'git -C "$tree" rm --quiet current/guides/acp/index.html\n' +
      'git -C "$tree" commit --quiet -m "docs: publish current"\n' +
      'git worktree remove "$tree"\n',
    { mode: 0o755 }
  );
  fs.writeFileSync(path.join(commands, 'python'), '#!/bin/sh\nexec python3 "$@"\n', {
    mode: 0o755,
  });
  const publish = publishStep('Publish Current or minor documentation locally');
  execute('bash', [], {
    cwd: repository,
    input: publish.run,
    env: {
      ...fixtureEnvironment,
      PATH: `${commands}${path.delimiter}${process.env.PATH}`,
      RUNNER_TEMP: temporary,
      DOCS_VERSION: 'current',
      UPDATE_STABLE: 'false',
      SOURCE_COMMIT: 'a'.repeat(40),
    },
  });

  const canonical = (route) =>
    git(['show', `gh-pages:v10.2/${route}index.html`]).match(/rel="canonical" href="([^"]*)"/)[1];
  assert.equal(canonical(''), 'https://zeroshot.sh/docs/current/');
  assert.equal(canonical('guides/acp/'), 'https://zeroshot.sh/docs/v10.2/guides/acp/');
  assert.equal(git(['worktree', 'list', '--porcelain']).match(/^worktree /gm).length, 1);
});
