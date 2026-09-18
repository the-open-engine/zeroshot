'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { after, describe, it } = require('node:test');

const {
  installSkills,
  managedSource,
  reportSkillResults,
  requireCompleteSkillInstall,
} = require('../../npm/zeroshot/lib/skills');

const temporaries = [];

function makeFixture(
  document = '---\nname: zeroshot\ndescription: Test skill\n---\n\nVersion one.\n'
) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-skill-test-'));
  temporaries.push(root);
  const packageRoot = path.join(root, 'package');
  const sourceDirectory = path.join(packageRoot, 'skills', 'zeroshot');
  const homeDirectory = path.join(root, 'home');
  fs.mkdirSync(sourceDirectory, { recursive: true });
  fs.mkdirSync(homeDirectory);
  fs.writeFileSync(path.join(sourceDirectory, 'SKILL.md'), document);
  return { document, homeDirectory, packageRoot, root };
}

function install(fixture, options = {}) {
  return installSkills({
    packageRoot: fixture.packageRoot,
    homeDirectory: fixture.homeDirectory,
    environment: {},
    uid: 1000,
    ...options,
  });
}

function destinations(fixture, claudeRoot = path.join(fixture.homeDirectory, '.claude')) {
  return {
    agents: path.join(fixture.homeDirectory, '.agents', 'skills', 'zeroshot', 'SKILL.md'),
    claude: path.join(claudeRoot, 'skills', 'zeroshot', 'SKILL.md'),
  };
}

function resultFor(results, id) {
  return results.find((result) => result.id === id);
}

function seedAgentSkill(fixture, document) {
  const filename = destinations(fixture).agents;
  fs.mkdirSync(path.dirname(filename), { recursive: true });
  fs.writeFileSync(filename, document);
  return filename;
}

after(() => {
  for (const temporary of temporaries) fs.rmSync(temporary, { recursive: true, force: true });
});

describe('npm Zeroshot skill installation', () => {
  it('installs one managed skill for Codex, Copilot, and Claude Code', () => {
    const fixture = makeFixture();
    const results = install(fixture);
    const files = destinations(fixture);

    assert.deepEqual(
      results.map(({ id, status }) => [id, status]),
      [
        ['agents', 'installed'],
        ['claude', 'installed'],
      ]
    );
    assert.equal(managedSource(fs.readFileSync(files.agents, 'utf8')), fixture.document);
    assert.equal(fs.readFileSync(files.agents, 'utf8'), fs.readFileSync(files.claude, 'utf8'));
    assert.match(fs.readFileSync(files.agents, 'utf8'), /^---[\s\S]+?---\n<!-- managed by /);
  });

  it('is byte-stable when reinstalled', () => {
    const fixture = makeFixture();
    install(fixture);
    const files = destinations(fixture);
    const before = fs.readFileSync(files.agents);
    const results = install(fixture);

    assert.deepEqual(
      results.map(({ status }) => status),
      ['unchanged', 'unchanged']
    );
    assert.deepEqual(fs.readFileSync(files.agents), before);
    for (const filename of Object.values(files)) {
      assert.deepEqual(
        fs.readdirSync(path.dirname(filename)).filter((entry) => entry.endsWith('.tmp')),
        []
      );
    }
  });

  it('updates an unmodified managed skill', () => {
    const fixture = makeFixture();
    install(fixture);
    const next = '---\nname: zeroshot\ndescription: Test skill\n---\n\nVersion two.\n';
    fs.writeFileSync(path.join(fixture.packageRoot, 'skills', 'zeroshot', 'SKILL.md'), next);

    const results = install(fixture);

    assert.deepEqual(
      results.map(({ status }) => status),
      ['updated', 'updated']
    );
    assert.equal(managedSource(fs.readFileSync(destinations(fixture).agents, 'utf8')), next);
  });

  it('adopts an exact unmanaged copy without changing its instructions', () => {
    const fixture = makeFixture();
    const filename = seedAgentSkill(fixture, fixture.document);

    const results = install(fixture);

    assert.equal(resultFor(results, 'agents').status, 'updated');
    assert.equal(managedSource(fs.readFileSync(filename, 'utf8')), fixture.document);
  });

  it('preserves unmanaged and user-modified skills as conflicts', () => {
    const fixture = makeFixture();
    const files = destinations(fixture);
    seedAgentSkill(fixture, 'user-owned\n');

    let results = install(fixture);
    assert.equal(resultFor(results, 'agents').status, 'conflict');
    assert.equal(fs.readFileSync(files.agents, 'utf8'), 'user-owned\n');

    fs.rmSync(path.dirname(files.agents), { recursive: true });
    install(fixture);
    fs.appendFileSync(files.agents, '\nUser edit.\n');
    const modified = fs.readFileSync(files.agents, 'utf8');
    results = install(fixture);
    assert.equal(resultFor(results, 'agents').status, 'conflict');
    assert.equal(fs.readFileSync(files.agents, 'utf8'), modified);
  });
});

describe('npm Zeroshot skill installation boundaries', () => {
  it('honors an absolute CLAUDE_CONFIG_DIR', () => {
    const fixture = makeFixture();
    const claudeRoot = path.join(fixture.root, 'custom-claude');
    const results = install(fixture, { environment: { CLAUDE_CONFIG_DIR: claudeRoot } });

    assert.equal(resultFor(results, 'claude').status, 'installed');
    assert.equal(
      managedSource(fs.readFileSync(destinations(fixture, claudeRoot).claude, 'utf8')),
      fixture.document
    );
    assert.equal(fs.existsSync(destinations(fixture).claude), false);
  });

  it('keeps one destination usable when another path fails', () => {
    const fixture = makeFixture();
    fs.writeFileSync(path.join(fixture.homeDirectory, '.claude'), 'not a directory');

    const results = install(fixture);

    assert.equal(resultFor(results, 'agents').status, 'installed');
    assert.equal(resultFor(results, 'claude').status, 'failed');
    assert.equal(fs.existsSync(destinations(fixture).agents), true);
  });

  it(
    'does not follow a conflicting skill directory symlink',
    { skip: process.platform === 'win32' },
    () => {
      const fixture = makeFixture();
      const files = destinations(fixture);
      const outside = path.join(fixture.root, 'outside');
      fs.mkdirSync(outside);
      fs.mkdirSync(path.dirname(path.dirname(files.agents)), { recursive: true });
      fs.symlinkSync(outside, path.dirname(files.agents), 'dir');

      const results = install(fixture);

      assert.equal(resultFor(results, 'agents').status, 'conflict');
      assert.equal(fs.existsSync(path.join(outside, 'SKILL.md')), false);
    }
  );

  it('rejects relative homes and Claude config roots instead of writing under cwd', () => {
    const fixture = makeFixture();
    let results = install(fixture, { homeDirectory: 'relative-home' });
    assert.deepEqual(
      results.map(({ status }) => status),
      ['failed', 'failed']
    );

    results = install(fixture, { environment: { CLAUDE_CONFIG_DIR: 'relative-claude' } });
    assert.equal(resultFor(results, 'agents').status, 'installed');
    assert.equal(resultFor(results, 'claude').status, 'failed');
  });

  it('does not guess the invoking user home under sudo', () => {
    const fixture = makeFixture();
    const results = install(fixture, { environment: { SUDO_USER: 'developer' }, uid: 0 });

    assert.deepEqual(
      results.map(({ status }) => status),
      ['failed', 'failed']
    );
    assert.equal(fs.existsSync(destinations(fixture).agents), false);
    assert.match(resultFor(results, 'agents').message, /user-owned npm prefix/);
  });

  it('reports partial installation without hiding successful agents', () => {
    const output = [];
    const errors = [];
    const results = [
      { label: 'Codex/GitHub Copilot', status: 'installed' },
      { label: 'Claude Code', status: 'failed', path: '/bad/path', message: 'denied' },
    ];
    const stdout = { write: (value) => output.push(value) };
    const stderr = { write: (value) => errors.push(value) };

    assert.equal(reportSkillResults(results, stdout, stderr), false);

    assert.match(output.join(''), /ready for Codex\/GitHub Copilot/);
    assert.match(errors.join(''), /Claude Code: \/bad\/path: denied/);
    assert.match(errors.join(''), /lifecycle scripts enabled/);
    assert.throws(
      () => requireCompleteSkillInstall(results, stdout, stderr),
      /SKILL_INSTALL_INCOMPLETE/
    );
  });
});
