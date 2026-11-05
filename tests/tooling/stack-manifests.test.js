'use strict';

// Stack manifests that the docs and the zeroshot-plan skill teach must pass `zeroshot stack validate`.
const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { after, it } = require('node:test');
const yaml = require('js-yaml');

const projectRoot = path.resolve(__dirname, '..', '..');
const STACK_SCHEMA = 'zeroshot.stack/v1';
const PLAN_TASK_SECTIONS = [
  'Goal',
  'Context',
  'Acceptance criteria',
  'Constraints',
  'Decided approach',
  'Decisions',
];
const temporaries = [];

after(() => {
  for (const directory of temporaries) fs.rmSync(directory, { recursive: true, force: true });
});

function read(relative) {
  return fs.readFileSync(path.join(projectRoot, relative), 'utf8');
}

function jsonBlocks(markdown) {
  return [...markdown.matchAll(/^( *)```json\n([\s\S]*?)^\1```$/gm)].map(([, indent, body]) =>
    JSON.parse(
      body
        .split('\n')
        .map((line) => line.slice(indent.length))
        .join('\n')
    )
  );
}

function stackManifests(markdown) {
  return jsonBlocks(markdown).filter((value) => value.schema === STACK_SCHEMA);
}

function tomorrow() {
  return new Date(Date.now() + 24 * 60 * 60 * 1000).toISOString();
}

function zeroshotStackValidate(manifest) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-stack-test-'));
  temporaries.push(directory);
  const file = path.join(directory, 'stack.json');
  fs.writeFileSync(file, JSON.stringify(manifest, null, 2));
  const command = ['run', '--quiet', '--locked', '--package', 'zeroshot', '--bin', 'zeroshot'];
  return spawnSync('cargo', [...command, '--', 'stack', 'validate', file], {
    cwd: projectRoot,
    encoding: 'utf8',
    maxBuffer: 16 * 1024 * 1024,
  });
}

function validated(manifest) {
  const result = zeroshotStackValidate(manifest);
  assert.equal(result.status, 0, result.stderr);
  return JSON.parse(result.stdout);
}

function parsePlan(markdown) {
  const match = /^---\n([\s\S]*?)\n---\n([\s\S]*)$/.exec(markdown);
  assert.ok(match, 'plans start with YAML frontmatter');
  const sections = new Map();
  for (const part of match[2].split(/^## /m).slice(1)) {
    const newline = part.indexOf('\n');
    sections.set(part.slice(0, newline).trim(), part.slice(newline + 1).trim());
  }
  return { ...yaml.load(match[1]), sections };
}

function planTask(plan) {
  return PLAN_TASK_SECTIONS.filter((name) => plan.sections.has(name))
    .map((name) => `## ${name}\n\n${plan.sections.get(name)}`)
    .join('\n\n');
}

// Follows the zeroshot-plan skill: one run per plan, its task sections as input, and `needs`
// from `depends_on`. Values the skill asks the user for are filled in as a user would.
function stackFromPlans(template, plans, delivery) {
  const ids = new Set(plans.map((plan) => plan.id));
  const needs = (plan) => (plan.depends_on ?? []).filter((id) => ids.has(id));
  const runs = Object.fromEntries(
    plans.map((plan) => {
      const run = { input: { task: planTask(plan) } };
      if (needs(plan).length > 0) run.needs = needs(plan);
      return [plan.id, run];
    })
  );
  const base = plans.find((plan) => needs(plan).length === 0);
  return {
    ...template,
    title: base.title,
    delivery,
    source: { repository: 'acme/shop', branch: 'main' },
    profile: 'org:software-change',
    expiresAt: tomorrow(),
    runs,
  };
}

it('builds a valid stack manifest from dependent plans as the zeroshot-plan skill describes', () => {
  const skill = read('npm/zeroshot/skills/zeroshot-plan/SKILL.md');
  const prose = skill.replace(/\s+/g, ' ');
  assert.ok(prose.includes('.zeroshot/plans/<id>.stack.json'));
  assert.ok(prose.includes(`${PLAN_TASK_SECTIONS.slice(0, -1).join(', ')} and Decisions`));
  const templates = stackManifests(skill);
  assert.equal(templates.length, 1, 'the skill documents one stack manifest');
  const [template] = templates;
  const [templateRun] = Object.values(template.runs);
  assert.deepEqual(Object.keys(templateRun).sort(), ['input', 'needs']);
  assert.deepEqual(Object.keys(templateRun.input), ['task']);

  const plans = ['order-totals.md', 'order-totals.1.md'].map((file) =>
    parsePlan(read(`tests/tooling/fixtures/stack-plans/${file}`))
  );
  const merge = stackFromPlans(template, plans, 'merge');
  assert.deepEqual(Object.keys(merge).sort(), Object.keys(template).sort());
  assert.deepEqual(merge.runs['order-totals.1'].needs, ['order-totals']);
  assert.match(merge.runs['order-totals.1'].input.task, /^## Goal\n[\s\S]*## Constraints\n/);
  assert.doesNotMatch(merge.runs['order-totals.1'].input.task, /Out of scope/);
  assert.deepEqual(validated(merge), { valid: true });
  assert.deepEqual(validated(stackFromPlans(template, plans, 'stacked_pull_requests')), {
    valid: true,
    order: ['order-totals', 'order-totals.1'],
  });

  // The skill leaves delivery to the user; its placeholder fails until they choose.
  const undecided = zeroshotStackValidate(stackFromPlans(template, plans, template.delivery));
  assert.notEqual(undecided.status, 0);
  assert.match(undecided.stderr, /delivery must be "merge" or "stacked_pull_requests"/);
});

it('validates the stack guide examples and prints their documented order', () => {
  const guide = read('docs/guides/stacks.md');
  const manifests = stackManifests(guide);
  assert.ok(manifests.length > 0, 'the guide shows a stack manifest');
  const documented = jsonBlocks(guide).filter((value) => Array.isArray(value.order));
  for (const manifest of manifests) {
    const result = validated({ ...manifest, expiresAt: tomorrow() });
    if (manifest.delivery === 'stacked_pull_requests') {
      assert.deepEqual([result], documented);
    } else {
      assert.deepEqual(result, { valid: true });
    }
  }
});
