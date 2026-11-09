'use strict';

const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { it } = require('node:test');
const yaml = require('js-yaml');

const { classifyPath, classifyPaths } = require('../../.github/ci-path-classifier');

const projectRoot = path.resolve(__dirname, '..', '..');
const classifierPath = path.join(projectRoot, '.github', 'ci-path-classifier.js');
const selections = (result) => {
  const { native, python, tooling, npm, docs } = result;
  return { native, python, tooling, npm, docs };
};

it('selects only lightweight checks for the root README', () => {
  const result = classifyPaths(['./README.md']);
  assert.equal(classifyPath('README.md'), 'readme');
  assert.deepEqual(selections(result), {
    native: false,
    python: false,
    tooling: false,
    npm: false,
    docs: false,
  });
  assert.equal(result.readme, true);
  assert.equal(result.readmeOnly, true);
});

it('never treats mixed changes or another README as root-README-only', () => {
  for (const pathname of [
    'zeroshot/src/main.rs',
    'tests/tooling/release-contract.test.js',
    'docs/concepts/targets.md',
    'npm/zeroshot/README.md',
    'sdks/python/README.md',
    'package-lock.json',
    '.opcore.json',
    '.github/workflows/opcore.yml',
    'unknown.file',
  ]) {
    const result = classifyPaths(['README.md', pathname]);
    assert.equal(result.readmeOnly, false, pathname);
    assert.equal(result.readme, true, pathname);
    const expected = classifyPaths([pathname]);
    assert.deepEqual(selections(result), selections(expected), pathname);
  }
  assert.equal(classifyPaths(['npm/zeroshot/README.md']).readmeOnly, false);
});

it('selects native checks and their established dependent lanes', () => {
  for (const pathname of [
    'zeroshot/src/main.rs',
    'ui/src/App.tsx',
    'ui/package-lock.json',
    '.dockerignore',
    'tests/tooling/smoke-ui.js',
  ]) {
    assert.equal(classifyPath(pathname), 'native');
  }
  assert.deepEqual(selections(classifyPaths(['zeroshot/src/main.rs'])), {
    native: true,
    python: true,
    tooling: true,
    npm: false,
    docs: false,
  });
});

it('runs the stack manifest tooling test when the stack validator changes', () => {
  for (const pathname of [
    'zeroshot/src/native_v2_cli/execution/stacks.rs',
    'crates/openengine-cluster-protocol/src/native_v2_hosted/stack.rs',
  ]) {
    assert.equal(classifyPath(pathname), 'native', pathname);
    assert.equal(classifyPaths([pathname]).tooling, true, pathname);
  }
});

it('keeps npm-only changes out of native, Python, and docs checks', () => {
  assert.equal(classifyPath('npm/zeroshot/install.js'), 'npm');
  assert.deepEqual(selections(classifyPaths(['npm/zeroshot/install.js'])), {
    native: false,
    python: false,
    tooling: false,
    npm: true,
    docs: false,
  });
});

it('runs package generation checks for canonical remote skills and both client catalogs', () => {
  for (const name of ['zeroshot', 'zeroshot-plan', 'zeroshot-graph-design']) {
    assert.deepEqual(selections(classifyPaths([`npm/zeroshot/skills/${name}/SKILL.md`])), {
      native: false,
      python: false,
      tooling: true,
      npm: true,
      docs: false,
    });
  }
  for (const filename of [
    'plugins/metadata.json',
    'plugins/zeroshot-openai/.codex-plugin/plugin.json',
    'plugins/zeroshot-claude/.mcp.json',
    '.agents/plugins/marketplace.json',
    '.claude-plugin/marketplace.json',
  ]) {
    assert.deepEqual(
      selections(classifyPaths([filename])),
      {
        native: false,
        python: false,
        tooling: true,
        npm: false,
        docs: false,
      },
      filename
    );
  }
  const workflow = yaml.load(
    fs.readFileSync(path.join(projectRoot, '.github/workflows/ci.yml'), 'utf8')
  );
  const quick = workflow.jobs['quick-check'].steps;
  assert.equal(
    quick.find((step) => step.uses?.startsWith('actions/checkout@')).with['fetch-depth'],
    0
  );
  const gate = quick.find((step) => step.name === 'Check generated plugins and content versions');
  assert.equal(gate.env.PLUGIN_BASE_SHA, '${{ github.event.pull_request.base.sha }}');
  assert.equal(gate.run, 'node scripts/plugins.js check --base "$PLUGIN_BASE_SHA"');
  assert.ok(quick.some((step) => step.run?.includes('tests/tooling/plugins.test.js')));
});

it('keeps ordinary documentation changes in the strict docs lane', () => {
  assert.equal(classifyPath('docs/concepts/targets.md'), 'docs');
  assert.deepEqual(selections(classifyPaths(['docs/concepts/targets.md'])), {
    native: false,
    python: false,
    tooling: false,
    npm: false,
    docs: true,
  });
});

it('runs strict docs alongside direct Python SDK changes', () => {
  assert.equal(classifyPath('sdks/python/src/zeroshot/client.py'), 'python');
  assert.deepEqual(selections(classifyPaths(['sdks/python/src/zeroshot/client.py'])), {
    native: false,
    python: true,
    tooling: false,
    npm: false,
    docs: true,
  });
});

it('preserves explicit cross-lane ownership', () => {
  const cases = new Map([
    [
      '.github/workflows/release-python.yml',
      { native: false, python: true, tooling: true, npm: false, docs: true },
    ],
    [
      'distribution/zeroshot-targets.json',
      { native: true, python: true, tooling: true, npm: true, docs: false },
    ],
    [
      'scripts/distribution/artifacts.js',
      { native: true, python: true, tooling: true, npm: true, docs: false },
    ],
    ['docs/zeroshot-cli.md', { native: true, python: true, tooling: true, npm: false, docs: true }],
    [
      'docs/getting-started/install.md',
      { native: false, python: false, tooling: false, npm: false, docs: true },
    ],
    ['package-lock.json', { native: false, python: false, tooling: true, npm: true, docs: false }],
    [
      'npm/zeroshot/package.json',
      { native: false, python: false, tooling: false, npm: true, docs: true },
    ],
    [
      'npm/zeroshot/README.md',
      { native: false, python: false, tooling: false, npm: true, docs: true },
    ],
    [
      'npm/zeroshot/skills/zeroshot-plan/SKILL.md',
      { native: false, python: false, tooling: true, npm: true, docs: false },
    ],
    [
      'docs/guides/stacks.md',
      { native: false, python: false, tooling: true, npm: false, docs: true },
    ],
  ]);
  for (const [pathname, expected] of cases) {
    assert.deepEqual(selections(classifyPaths([pathname])), expected, pathname);
  }
});

it('runs every lane for release, CI, unknown, or unresolvable changes', () => {
  for (const paths of [
    ['.github/workflows/ci.yml'],
    ['.github/workflows/opcore.yml'],
    ['.github/select-ci-lanes.sh'],
    ['.github/workflows/release.yml'],
    ['new-surface/file'],
    [],
  ]) {
    assert.deepEqual(selections(classifyPaths(paths)), {
      native: true,
      python: true,
      tooling: true,
      npm: true,
      docs: true,
    });
    assert.equal(classifyPaths(paths).readme, true);
    assert.equal(classifyPaths(paths).readmeOnly, false);
  }
});

it('emits every GitHub output from null-delimited paths', () => {
  const result = spawnSync(process.execPath, [classifierPath], {
    cwd: projectRoot,
    encoding: 'utf8',
    input: 'npm/zeroshot/install.js\0docs/concepts/targets.md\0',
  });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(
    result.stdout,
    'native=false\npython=false\ntooling=false\nnpm=true\ndocs=true\nreadme=false\nreadme_only=false\n'
  );
});

it(
  'classifies real Git history and falls back to full checks when history is unavailable',
  {
    skip: process.platform === 'win32',
  },
  (t) => {
    const repo = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-ci-paths-'));
    t.after(() => fs.rmSync(repo, { recursive: true, force: true }));
    // Git hooks export checkout/index locations. Never carry those into the fixture.
    const environment = Object.fromEntries(
      Object.entries(process.env).filter(([name]) => !name.startsWith('GIT_'))
    );
    fs.mkdirSync(path.join(repo, '.github'));
    fs.copyFileSync(classifierPath, path.join(repo, '.github/ci-path-classifier.js'));
    const git = (...args) => {
      const result = spawnSync('git', args, { cwd: repo, encoding: 'utf8', env: environment });
      assert.equal(result.status, 0, result.stderr);
      return result.stdout.trim();
    };
    git('init');
    git('config', 'user.name', 'CI fixture');
    git('config', 'user.email', 'ci@example.invalid');
    git('config', 'core.hooksPath', '/dev/null');
    fs.writeFileSync(path.join(repo, 'README.md'), 'Before\n');
    git('add', 'README.md', '.github/ci-path-classifier.js');
    git('commit', '-m', 'initial');
    const base = git('rev-parse', 'HEAD');
    fs.writeFileSync(path.join(repo, 'README.md'), 'After\n');
    git('commit', '-am', 'readme');
    const head = git('rev-parse', 'HEAD');
    const select = (overrides = {}) => {
      const result = spawnSync('bash', [path.join(projectRoot, '.github/select-ci-lanes.sh')], {
        cwd: repo,
        encoding: 'utf8',
        env: {
          ...environment,
          EVENT_NAME: 'pull_request',
          BASE_SHA: base,
          HEAD_SHA: head,
          ...overrides,
        },
      });
      assert.equal(result.status, 0, result.stderr);
      return Object.fromEntries(
        result.stdout
          .trim()
          .split('\n')
          .map((line) => line.split('='))
      );
    };
    for (const event of ['pull_request', 'push', 'merge_group']) {
      assert.equal(select({ EVENT_NAME: event }).readme_only, 'true');
    }
    for (const overrides of [
      { EVENT_NAME: 'workflow_dispatch' },
      { BASE_SHA: '' },
      { BASE_SHA: '0'.repeat(40) },
      { BASE_SHA: 'missing' },
      { HEAD_SHA: 'missing' },
      { HEAD_SHA: base },
    ]) {
      const result = select(overrides);
      assert.equal(result.readme_only, 'false');
      for (const lane of ['native', 'python', 'tooling', 'npm', 'docs', 'readme']) {
        assert.equal(result[lane], 'true', JSON.stringify(overrides));
      }
    }
    git('mv', 'README.md', 'unknown.file');
    git('commit', '-m', 'rename');
    const renamed = select({ BASE_SHA: head, HEAD_SHA: git('rev-parse', 'HEAD') });
    assert.equal(renamed.readme_only, 'false');
    assert.equal(renamed.native, 'true');
  }
);

it('makes README checks mandatory and gates heavy Opcore steps with the shared classifier', () => {
  const readWorkflow = (name) =>
    yaml.load(fs.readFileSync(path.join(projectRoot, '.github/workflows', name), 'utf8'));
  const ci = readWorkflow('ci.yml');
  const opcore = readWorkflow('opcore.yml');
  const readme = ci.jobs['readme-check'];
  assert.equal(readme.if, "needs.classify.outputs.readme == 'true'");
  assert.ok(ci.jobs.required.needs.includes('readme-check'));
  assert.equal(ci.jobs.required.steps[0].env.README_RESULT, '${{ needs.readme-check.result }}');
  const commands = readme.steps.map((step) => step.run ?? '').join('\n');
  assert.match(commands, /prettier --check README\.md/);
  assert.match(commands, /node --test tests\/tooling\/docs-agent-contract\.test\.js/);
  assert.doesNotMatch(commands, /cargo|protocol:check|distribution:check|npm test/);
  const select = (steps) => steps.find((step) => step.name === 'Select CI lanes');
  assert.deepEqual(select(opcore.jobs.check.steps), select(ci.jobs.classify.steps));
  const heavy = opcore.jobs.check.steps.filter(
    (step) => step.uses?.startsWith('dtolnay/') || /opcore|cargo/.test(step.run ?? '')
  );
  assert.equal(heavy.length, 5);
  for (const step of heavy) assert.equal(step.if, "steps.paths.outputs.readme_only != 'true'");
});

it(
  'fails the required gate when a selected README check fails or is missing',
  {
    skip: process.platform === 'win32',
  },
  () => {
    const ci = yaml.load(
      fs.readFileSync(path.join(projectRoot, '.github/workflows/ci.yml'), 'utf8')
    );
    const gate = ci.jobs.required.steps[0];
    const environment = Object.fromEntries(
      Object.keys(gate.env).map((key) => [key, key.endsWith('_SELECTED') ? 'false' : 'skipped'])
    );
    Object.assign(environment, {
      CLASSIFY_RESULT: 'success',
      README_SELECTED: 'true',
      EVENT_NAME: 'merge_group',
      OPCORE_RESULT: 'success',
    });
    for (const status of ['success', 'failure', 'cancelled', 'skipped', '']) {
      const result = spawnSync('bash', ['-c', gate.run], {
        encoding: 'utf8',
        env: { ...process.env, ...environment, README_RESULT: status },
      });
      assert.equal(result.status === 0, status === 'success', result.stderr);
    }
  }
);
