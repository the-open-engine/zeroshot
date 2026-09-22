'use strict';

const assert = require('node:assert/strict');
const { it } = require('node:test');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const { createArchive, sha256 } = require('../../scripts/distribution');

const root = path.resolve(__dirname, '..', '..');

function runNpm(args) {
  const result = spawnSync('npm', args, {
    cwd: root,
    encoding: 'utf8',
    shell: process.platform === 'win32',
  });
  assert.equal(result.status, 0, result.error?.message ?? result.stdout + result.stderr);
  return result.stdout;
}

function stagePackedPackage(temporary) {
  const stagedPackage = path.join(temporary, 'package');
  const packedDirectory = path.join(temporary, 'packed');
  const installDirectory = path.join(temporary, 'installed');
  const homeDirectory = path.join(temporary, 'home');
  fs.cpSync(path.join(root, 'npm', 'zeroshot'), stagedPackage, { recursive: true });
  fs.mkdirSync(packedDirectory);
  fs.mkdirSync(homeDirectory);

  const version = '8.0.0';
  const manifestPath = path.join(stagedPackage, 'package.json');
  const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
  fs.writeFileSync(manifestPath, `${JSON.stringify({ ...manifest, version }, null, 2)}\n`);

  const [packed] = JSON.parse(
    runNpm([
      'pack',
      stagedPackage,
      '--json',
      '--ignore-scripts',
      '--pack-destination',
      packedDirectory,
    ])
  );
  const packedFiles = new Set(packed.files.map(({ path: filename }) => filename));
  for (const filename of [
    'README.md',
    'bin/zeroshot.js',
    'install.js',
    'lib/install.js',
    'lib/release-artifacts.js',
    'lib/skills.js',
    'package.json',
    'skills/zeroshot/SKILL.md',
    'targets.json',
  ]) {
    assert.equal(packedFiles.has(filename), true, `${filename} is missing from npm pack`);
  }
  assert.equal(
    [...packedFiles].some((filename) => filename.startsWith('bin/native/')),
    false
  );

  const tarball = path.join(packedDirectory, packed.filename);
  runNpm([
    'install',
    '--ignore-scripts',
    '--no-audit',
    '--no-fund',
    '--prefix',
    installDirectory,
    tarball,
  ]);
  const packageRoot = path.join(
    installDirectory,
    'node_modules',
    '@the-open-engine-company',
    'zeroshot'
  );
  return { homeDirectory, packageRoot, version };
}

it('packs and installs the published npm surface on the current host', async (t) => {
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-npm-package-'));
  t.after(() => fs.rmSync(temporary, { recursive: true, force: true }));
  const { homeDirectory, packageRoot, version } = stagePackedPackage(temporary);
  const installed = require(path.join(packageRoot, 'lib', 'install.js'));
  const skills = require(path.join(packageRoot, 'lib', 'skills.js'));

  const selected = installed.selectTarget();
  const binary = Buffer.from(`zeroshot package smoke for ${process.platform}/${process.arch}\n`);
  const restic = Buffer.from(`restic package smoke for ${process.platform}/${process.arch}\n`);
  const archive = createArchive([
    { name: selected.executable, contents: binary },
    { name: selected.resticExecutable, contents: restic },
  ]);
  const filename = installed.archiveName(version, selected.target);
  const checksum = Buffer.from(`${sha256(archive)}  ${filename}\n`);
  const requested = [];
  const skillRuns = [];
  const sink = { write() {} };
  const installOptions = {
    packageRoot,
    packageMetadata: { version },
    homeDirectory,
    environment: {},
    uid: 1000,
    fetchBuffer: async (url) => {
      requested.push(url);
      return url.endsWith('/SHA256SUMS') ? checksum : archive;
    },
    onSkillResults: (results) => {
      skillRuns.push(results);
      skills.requireCompleteSkillInstall(results, sink, sink);
    },
  };
  const destination = await installed.install(installOptions);

  assert.deepEqual(fs.readFileSync(destination), binary);
  assert.deepEqual(
    fs.readFileSync(path.join(path.dirname(destination), selected.resticExecutable)),
    restic
  );
  assert.deepEqual(
    skillRuns[0].map(({ id, status }) => [id, status]),
    [
      ['agents', 'installed'],
      ['claude', 'installed'],
    ]
  );
  const agentSkill = path.join(homeDirectory, '.agents', 'skills', 'zeroshot', 'SKILL.md');
  const before = fs.readFileSync(agentSkill);
  assert.equal(await installed.install(installOptions), destination);
  assert.deepEqual(
    skillRuns[1].map(({ status }) => status),
    ['unchanged', 'unchanged']
  );
  assert.deepEqual(fs.readFileSync(agentSkill), before);

  fs.appendFileSync(agentSkill, '\nUser edit.\n');
  const edited = fs.readFileSync(agentSkill);
  await assert.rejects(installed.install(installOptions), /SKILL_INSTALL_INCOMPLETE/);
  assert.deepEqual(
    skillRuns[2].map(({ status }) => status),
    ['conflict', 'unchanged']
  );
  assert.deepEqual(fs.readFileSync(agentSkill), edited);
  assert.deepEqual(
    requested.map((url) => url.split('/').at(-1)),
    ['SHA256SUMS', filename, 'SHA256SUMS', filename, 'SHA256SUMS', filename]
  );
});
