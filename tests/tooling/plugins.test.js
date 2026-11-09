'use strict';

const assert = require('node:assert/strict');
const { execFileSync, spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { after, it } = require('node:test');

const {
  buildFiles,
  check,
  checkVersion,
  generate,
  packagePlugins,
} = require('../../scripts/plugins');

const repositoryRoot = path.resolve(__dirname, '../..');
const roots = ['plugins/zeroshot-openai', 'plugins/zeroshot-claude'];
const skills = ['zeroshot', 'zeroshot-plan', 'zeroshot-graph-design'];
const temporaries = [];
const localGitVariables = execFileSync('git', ['rev-parse', '--local-env-vars'], {
  encoding: 'utf8',
})
  .trim()
  .split('\n');

function gitEnvironment() {
  const env = { ...process.env, GIT_CONFIG_NOSYSTEM: '1' };
  // Keep each fixture independent of a calling hook's Git directory and index.
  for (const name of localGitVariables) delete env[name];
  return env;
}

function fixture() {
  const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-plugin-test-')));
  temporaries.push(root);
  const sources = [
    'plugins/metadata.json',
    'plugins/assets/zeroshot-seal.png',
    'LICENSE',
    ...skills.map((name) => `npm/zeroshot/skills/${name}/SKILL.md`),
  ];
  for (const relative of sources) {
    fs.mkdirSync(path.dirname(path.join(root, relative)), { recursive: true });
    fs.copyFileSync(path.join(repositoryRoot, relative), path.join(root, relative));
  }
  generate(root);
  return root;
}

function updateMetadata(root, change) {
  const filename = path.join(root, 'plugins/metadata.json');
  const value = JSON.parse(fs.readFileSync(filename));
  fs.writeFileSync(filename, `${JSON.stringify({ ...value, ...change }, null, 2)}\n`);
}

function git(root, ...args) {
  return execFileSync('git', ['-C', root, ...args], {
    env: gitEnvironment(),
    stdio: ['ignore', 'pipe', 'pipe'],
  })
    .toString()
    .trim();
}

function commit(root, ...files) {
  git(root, 'add', '--', ...files);
  git(
    root,
    '-c',
    'user.name=Plugin fixture',
    '-c',
    'user.email=fixture@example.invalid',
    '-c',
    'commit.gpgsign=false',
    '-c',
    `core.hooksPath=${path.join(root, '.no-hooks')}`,
    'commit',
    '-qm',
    'fixture'
  );
  return git(root, 'rev-parse', 'HEAD');
}

function snapshotTree(directory, prefix = '') {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const relative = path.join(prefix, entry.name);
    const filename = path.join(directory, entry.name);
    return entry.isDirectory()
      ? [[relative, 'directory'], ...snapshotTree(filename, relative)]
      : [[relative, fs.readFileSync(filename)]];
  });
}

after(() => {
  for (const root of temporaries) fs.rmSync(root, { recursive: true, force: true });
});

it('keeps both checked-in plugin packages and catalogs synchronized with canonical sources', () => {
  assert.equal(check(repositoryRoot).size, 18);
});

it('copies exactly the supported canonical skills and produces native client configurations', () => {
  const root = fixture();
  for (const directory of roots) {
    assert.deepEqual(
      fs.readdirSync(path.join(root, directory, 'skills')).sort(),
      [...skills].sort()
    );
    for (const name of skills) {
      assert.deepEqual(
        fs.readFileSync(path.join(root, directory, 'skills', name, 'SKILL.md')),
        fs.readFileSync(path.join(root, 'npm/zeroshot/skills', name, 'SKILL.md'))
      );
    }
    assert.equal(fs.existsSync(path.join(root, directory, 'bin')), false);
    assert.equal(fs.existsSync(path.join(root, directory, 'package.json')), false);
  }
  const openai = JSON.parse(fs.readFileSync(path.join(root, roots[0], '.mcp.json')));
  const claude = JSON.parse(fs.readFileSync(path.join(root, roots[1], '.mcp.json')));
  assert.equal(openai.mcpServers.zeroshot.type, 'http');
  assert.equal(claude.mcpServers.zeroshot.type, 'http');
  assert.equal(openai.mcpServers.zeroshot.url, claude.mcpServers.zeroshot.url);
  assert.deepEqual(openai.mcpServers.zeroshot.extensions['com.openai'].auth, {
    type: 'oauth',
    client: { mode: 'dcr' },
  });
  const manifest = JSON.parse(
    fs.readFileSync(path.join(root, roots[0], '.codex-plugin/plugin.json'))
  );
  assert.equal(manifest.mcpServers, './.mcp.json');
  assert.equal(manifest.skills, './skills/');
  assert.equal(fs.existsSync(path.join(root, roots[0], 'plugin.json')), false);
});

it('detects generated edits, missing files and unexpected private files', () => {
  const root = fixture();
  const readme = path.join(root, roots[0], 'README.md');
  fs.appendFileSync(readme, 'edited\n');
  assert.throws(() => check(root), /generated drift/);
  generate(root);
  fs.rmSync(readme);
  assert.throws(() => check(root), /missing or unexpected/);
  generate(root);
  fs.writeFileSync(path.join(root, roots[0], 'private-fixture.json'), '{"private":true}');
  assert.throws(() => check(root), /missing or unexpected/);
  generate(root);
  assert.doesNotThrow(() => check(root));
});

it('rejects symlinked sources, output files and parent directories before writing', () => {
  const root = fixture();
  const readme = path.join(root, roots[0], 'README.md');
  fs.rmSync(readme);
  fs.symlinkSync(path.join(root, 'LICENSE'), readme);
  assert.throws(() => check(root), /symlink/);
  assert.throws(() => generate(root), /symlink/);
  fs.rmSync(readme);
  generate(root);
  const assets = path.join(root, 'plugins/assets');
  fs.renameSync(assets, `${assets}-saved`);
  fs.symlinkSync(`${assets}-saved`, assets, 'dir');
  assert.throws(() => buildFiles(root), /symlink/);
});

it('rejects escaping icon paths, invalid icons and credential-bearing public URLs', () => {
  const root = fixture();
  updateMetadata(root, { icon: '../icon.png' });
  assert.throws(() => buildFiles(root), /escapes/);
  updateMetadata(root, { icon: 'plugins/assets/zeroshot-seal.png' });
  fs.writeFileSync(path.join(root, 'plugins/assets/zeroshot-seal.png'), 'not a png');
  assert.throws(() => buildFiles(root), /must be a PNG/);
  fs.copyFileSync(
    path.join(repositoryRoot, 'plugins/assets/zeroshot-seal.png'),
    path.join(root, 'plugins/assets/zeroshot-seal.png')
  );
  updateMetadata(root, { supportURL: 'https://user:secret@example.invalid/support' });
  assert.throws(() => buildFiles(root), /invalid public supportURL/);
});

it('requires a real base commit and proves first introduction from that exact tree', () => {
  const root = fixture();
  git(root, 'init', '--quiet');
  const base = commit(root, 'LICENSE');
  assert.doesNotThrow(() => checkVersion(root, base));
  assert.throws(() => checkVersion(root, 'main'), /exact full commit/);
  assert.throws(() => checkVersion(root, '0'.repeat(40)), /base history is unavailable/);
  assert.throws(
    () => checkVersion(root, git(root, 'rev-parse', `${base}:LICENSE`)),
    /must name a commit/
  );
  const incomplete = commit(root, roots[0]);
  assert.throws(() => checkVersion(root, incomplete), /cannot prove first introduction/);
});

it('requires greater plugin versions for changed payloads and permits version-only increases', () => {
  const root = fixture();
  git(root, 'init', '--quiet');
  const base = commit(root, '.');
  assert.doesNotThrow(() => checkVersion(root, base));
  const original = JSON.parse(fs.readFileSync(path.join(root, 'plugins/metadata.json'))).version;
  const [major, minor, patch] = original.split('.').map(Number);
  updateMetadata(root, { version: `${major}.${minor}.${patch + 1}` });
  generate(root);
  assert.doesNotThrow(() => checkVersion(root, base));
  updateMetadata(root, { version: original });
  const canonical = path.join(root, 'npm/zeroshot/skills/zeroshot/SKILL.md');
  fs.appendFileSync(canonical, '\nChanged task guidance with its own version field.\n');
  generate(root);
  assert.throws(() => checkVersion(root, base), /strictly greater/);
  updateMetadata(root, { version: '0.0.0' });
  generate(root);
  assert.throws(() => checkVersion(root, base), /must not decrease/);
  updateMetadata(root, { version: `${major}.${minor}.${patch + 1}` });
  generate(root);
  assert.doesNotThrow(() => checkVersion(root, base));
});

it('isolates fixture writes and plugin history reads from inherited foreign Git context', () => {
  const foreign = fixture();
  fs.writeFileSync(path.join(foreign, 'outside-marker.txt'), 'Foreign repository fixture.\n');
  git(foreign, 'init', '--quiet');
  commit(foreign, '.');
  const root = fixture();
  git(root, 'init', '--quiet');
  const base = commit(root, '.');
  const before = snapshotTree(foreign);
  const env = {
    ...process.env,
    GIT_DIR: path.join(foreign, '.git'),
    GIT_COMMON_DIR: path.join(foreign, '.git'),
    GIT_WORK_TREE: foreign,
    GIT_INDEX_FILE: path.join(foreign, '.git/index'),
  };
  const fixtures = spawnSync(
    process.execPath,
    [
      '--test',
      '--test-name-pattern',
      'requires a real base commit|requires greater plugin versions',
      __filename,
    ],
    { env, encoding: 'utf8' }
  );
  const history = spawnSync(
    process.execPath,
    [
      '-e',
      'require(process.argv[1]).checkVersion(process.argv[2], process.argv[3])',
      path.join(repositoryRoot, 'scripts/plugins.js'),
      root,
      base,
    ],
    { env, encoding: 'utf8' }
  );
  assert.deepEqual(
    snapshotTree(foreign),
    before,
    'foreign repository must stay byte-for-byte unchanged'
  );
  assert.equal(fixtures.status, 0, fixtures.stderr || fixtures.stdout);
  assert.equal(history.status, 0, history.stderr || history.stdout);
});

it('rejects invalid release versions and changes to stable identities', () => {
  const root = fixture();
  for (const version of ['01.2.3', '1.2', '1.2.3-dev', '1.2.3+build', 1]) {
    updateMetadata(root, { version });
    assert.throws(() => buildFiles(root), /release SemVer/);
  }
  updateMetadata(root, { version: '0.1.0', name: 'renamed' });
  assert.throws(() => buildFiles(root), /identities are permanent/);
});

it('builds deterministic ZIPs that an independent ZIP reader extracts without repository access', () => {
  const root = fixture();
  const artifacts = packagePlugins(root);
  assert.deepEqual(packagePlugins(root), artifacts);
  const python = process.platform === 'win32' ? 'python' : 'python3';
  for (const [index, artifact] of artifacts.entries()) {
    const contents = JSON.parse(
      execFileSync(python, [
        '-c',
        'import base64,json,sys,zipfile\nwith zipfile.ZipFile(sys.argv[1]) as z:\n assert z.testzip() is None\n print(json.dumps({n:base64.b64encode(z.read(n)).decode() for n in z.namelist()}))',
        artifact.path,
      ])
    );
    const expected = [...check(root)].filter(([name]) => name.startsWith(`${roots[index]}/`));
    assert.deepEqual(
      Object.keys(contents).sort(),
      expected.map(([name]) => name.slice(roots[index].length + 1)).sort()
    );
    for (const [name, bytes] of expected) {
      assert.deepEqual(Buffer.from(contents[name.slice(roots[index].length + 1)], 'base64'), bytes);
    }
  }
});

it('never writes ZIPs into plugin sources or through a symlinked output location', () => {
  const root = fixture();
  assert.throws(() => packagePlugins(root, path.join(root, roots[0])), /overlaps/);
  for (const protectedRoot of ['plugins', '.agents', '.claude-plugin', 'npm/zeroshot/skills']) {
    assert.throws(
      () => packagePlugins(root, path.join(root, protectedRoot, '..archive-output')),
      /overlaps/
    );
  }
  const link = path.join(root, 'linked-output');
  fs.symlinkSync(path.join(root, 'plugins/assets'), link, 'dir');
  assert.throws(() => packagePlugins(root, link), /symlink/);
});

it('refuses an archive-file symlink without changing the unrelated target', () => {
  const root = fixture();
  const [artifact] = packagePlugins(root);
  const protectedFile = path.join(root, 'protected-output.txt');
  fs.writeFileSync(protectedFile, 'keep this file unchanged\n');
  fs.rmSync(artifact.path);
  fs.symlinkSync(protectedFile, artifact.path);
  assert.throws(() => packagePlugins(root), /symlink/);
  assert.equal(fs.readFileSync(protectedFile, 'utf8'), 'keep this file unchanged\n');
});
