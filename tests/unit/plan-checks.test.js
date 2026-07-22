const assert = require('assert');

const Ledger = require('../../src/ledger');
const MessageBus = require('../../src/message-bus');
const { PLAN_CHECK_TOPIC, checkPlan, attachPlanChecker } = require('../../src/plan-checks');

function makeBus() {
  const ledger = new Ledger(':memory:');
  const messageBus = new MessageBus(ledger);
  return { ledger, messageBus };
}

function makeCluster(clusterId, config = {}) {
  return { id: clusterId, config, agents: [] };
}

function publishPlan(messageBus, clusterId, data) {
  return messageBus.publish({
    cluster_id: clusterId,
    topic: 'PLAN_READY',
    sender: 'planner',
    content: { text: 'plan', data },
  });
}

describe('plan-checks: checkPlan', function () {
  it('passes a well-formed plan with no findings', function () {
    const { findings } = checkPlan({
      steps: [
        { id: 's1', kind: 'inspect', target: 'src/parser.js' },
        {
          id: 's2',
          kind: 'edit',
          target: 'src/parser.js',
          verify: 'npm test -- parser',
          dependsOn: ['s1'],
        },
        { id: 's3', kind: 'verify', target: 'npm test', dependsOn: ['s2'] },
      ],
      filesAffected: ['src/parser.js'],
    });
    assert.deepStrictEqual(findings, []);
  });

  it('scope-containment: accepts exact file match and directory prefix', function () {
    const { findings } = checkPlan({
      steps: [
        { id: 's1', kind: 'edit', target: 'src/parser.js', verify: 'npm test' },
        { id: 's2', kind: 'create', target: 'src/nested/new-file.js', verify: 'npm test' },
      ],
      filesAffected: ['src/parser.js', 'src'],
    });
    assert.deepStrictEqual(findings, []);
  });

  it('scope-containment: flags a mutating target outside filesAffected', function () {
    const { findings } = checkPlan({
      steps: [
        { id: 's1', kind: 'edit', target: 'src/parser.js', verify: 'npm test' },
        { id: 's2', kind: 'edit', target: 'docs/readme.md', verify: 'npm test' },
      ],
      filesAffected: ['src/parser.js'],
    });
    assert.strictEqual(findings.length, 1);
    assert.strictEqual(findings[0].rule, 'scope-containment');
    assert.strictEqual(findings[0].severity, 'warning');
    assert.strictEqual(findings[0].step_id, 's2');
    assert.match(findings[0].message, /docs\/readme\.md/);
  });

  it('scope-containment: skips non-mutating steps and missing filesAffected', function () {
    const inspectOnly = checkPlan({
      steps: [{ id: 's1', kind: 'inspect', target: 'anywhere/anything.js' }],
      filesAffected: ['src/parser.js'],
    });
    assert.deepStrictEqual(inspectOnly.findings, []);

    const noDeclaration = checkPlan({
      steps: [{ id: 's1', kind: 'edit', target: 'anywhere/anything.js', verify: 'x' }],
      filesAffected: [],
    });
    assert.deepStrictEqual(noDeclaration.findings, []);

    const missingDeclaration = checkPlan({
      steps: [{ id: 's1', kind: 'edit', target: 'anywhere/anything.js', verify: 'x' }],
    });
    assert.deepStrictEqual(missingDeclaration.findings, []);
  });

  it('scope-containment: sibling directory with shared prefix is not covered', function () {
    const { findings } = checkPlan({
      steps: [{ id: 's1', kind: 'edit', target: 'src/parser-utils/helper.js', verify: 'x' }],
      filesAffected: ['src/parser'],
    });
    assert.strictEqual(findings.length, 1);
    assert.strictEqual(findings[0].rule, 'scope-containment');
  });

  it('edit-requires-verify: satisfied by own verify field or a later verify step', function () {
    const ownField = checkPlan({
      steps: [{ id: 's1', kind: 'edit', target: 'a.js', verify: 'npm test' }],
    });
    assert.deepStrictEqual(ownField.findings, []);

    const laterStep = checkPlan({
      steps: [
        { id: 's1', kind: 'edit', target: 'a.js' },
        { id: 's2', kind: 'verify', target: 'npm test' },
      ],
    });
    assert.deepStrictEqual(laterStep.findings, []);
  });

  it('edit-requires-verify: flags mutation with no verification after it', function () {
    const { findings } = checkPlan({
      steps: [
        { id: 's1', kind: 'verify', target: 'npm test' }, // earlier verify does not count
        { id: 's2', kind: 'edit', target: 'a.js' },
      ],
    });
    assert.strictEqual(findings.length, 1);
    assert.strictEqual(findings[0].rule, 'edit-requires-verify');
    assert.strictEqual(findings[0].step_id, 's2');
  });

  it('step-shape: flags missing id, duplicate id, unknown kind, dangling dependsOn', function () {
    const { findings } = checkPlan({
      steps: [
        { kind: 'edit', target: 'a.js', verify: 'x' },
        { id: 's1', kind: 'edit', target: 'b.js', verify: 'x' },
        { id: 's1', kind: 'run', target: 'npm test' },
        { id: 's2', kind: 'frobnicate' },
        { id: 's3', kind: 'inspect', dependsOn: ['ghost'] },
      ],
    });
    assert.strictEqual(findings.length, 4);
    assert.ok(findings.every((f) => f.rule === 'step-shape'));
    assert.match(findings[0].message, /missing its id/);
    assert.match(findings[1].message, /duplicate step id 's1'/);
    assert.match(findings[2].message, /unknown kind 'frobnicate'/);
    assert.match(findings[3].message, /unknown step id 'ghost'/);
  });

  it('step-shape: reports duplicate before unknown kind for the same step', function () {
    const { findings } = checkPlan({
      steps: [
        { id: 's1', kind: 'edit', verify: 'x' },
        { id: 's1', kind: 'bogus' },
      ],
    });
    assert.strictEqual(findings.length, 2);
    assert.match(findings[0].message, /duplicate step id 's1'/);
    assert.match(findings[1].message, /unknown kind 'bogus'/);
  });
});

describe('plan-checks: attachPlanChecker', function () {
  it('publishes PLAN_CHECK_RESULT with findings for a bad plan', function () {
    const { ledger, messageBus } = makeBus();
    const cluster = makeCluster('pc-1');
    const detach = attachPlanChecker({ messageBus, cluster });

    const plan = publishPlan(messageBus, cluster.id, {
      steps: [{ id: 's1', kind: 'edit', target: 'secret/keys.js' }],
      filesAffected: ['src/app.js'],
    });

    const results = messageBus.query({ cluster_id: cluster.id, topic: PLAN_CHECK_TOPIC });
    assert.strictEqual(results.length, 1);
    assert.strictEqual(results[0].sender, 'plan-checker');
    assert.strictEqual(results[0].content.data.plan_message_id, plan.id);
    assert.strictEqual(results[0].content.data.severity, 'warning');
    const rules = results[0].content.data.findings.map((f) => f.rule);
    assert.ok(rules.includes('scope-containment'));
    assert.ok(rules.includes('edit-requires-verify'));
    assert.strictEqual(results[0].metadata.source, 'plan_checks');

    detach();
    ledger.close();
  });

  it('stays silent for a good plan', function () {
    const { ledger, messageBus } = makeBus();
    const cluster = makeCluster('pc-2');
    const detach = attachPlanChecker({ messageBus, cluster });

    publishPlan(messageBus, cluster.id, {
      steps: [{ id: 's1', kind: 'edit', target: 'src/app.js', verify: 'npm test' }],
      filesAffected: ['src/app.js'],
    });

    const results = messageBus.query({ cluster_id: cluster.id, topic: PLAN_CHECK_TOPIC });
    assert.strictEqual(results.length, 0);

    detach();
    ledger.close();
  });

  it('stays silent for prose-only plans (no steps, null steps, empty steps)', function () {
    const { ledger, messageBus } = makeBus();
    const cluster = makeCluster('pc-3');
    const detach = attachPlanChecker({ messageBus, cluster });

    publishPlan(messageBus, cluster.id, { summary: 'prose only' });
    publishPlan(messageBus, cluster.id, { steps: null });
    publishPlan(messageBus, cluster.id, { steps: [] });

    const results = messageBus.query({ cluster_id: cluster.id, topic: PLAN_CHECK_TOPIC });
    assert.strictEqual(results.length, 0);

    detach();
    ledger.close();
  });

  it('ignores PLAN_READY from other clusters', function () {
    const { ledger, messageBus } = makeBus();
    const cluster = makeCluster('pc-4');
    const detach = attachPlanChecker({ messageBus, cluster });

    publishPlan(messageBus, 'some-other-cluster', {
      steps: [{ id: 's1', kind: 'edit', target: 'secret/keys.js' }],
      filesAffected: ['src/app.js'],
    });

    const results = messageBus.query({ cluster_id: 'some-other-cluster', topic: PLAN_CHECK_TOPIC });
    assert.strictEqual(results.length, 0);

    detach();
    ledger.close();
  });

  it('opts out when cluster.config.planChecks.enabled === false', function () {
    const { ledger, messageBus } = makeBus();
    const cluster = makeCluster('pc-5', { planChecks: { enabled: false } });
    const detach = attachPlanChecker({ messageBus, cluster });

    publishPlan(messageBus, cluster.id, {
      steps: [{ id: 's1', kind: 'edit', target: 'secret/keys.js' }],
      filesAffected: ['src/app.js'],
    });

    const results = messageBus.query({ cluster_id: cluster.id, topic: PLAN_CHECK_TOPIC });
    assert.strictEqual(results.length, 0);

    detach();
    ledger.close();
  });

  it('stops checking after unsubscribe', function () {
    const { ledger, messageBus } = makeBus();
    const cluster = makeCluster('pc-6');
    const detach = attachPlanChecker({ messageBus, cluster });
    detach();

    publishPlan(messageBus, cluster.id, {
      steps: [{ id: 's1', kind: 'edit', target: 'secret/keys.js' }],
      filesAffected: ['src/app.js'],
    });

    const results = messageBus.query({ cluster_id: cluster.id, topic: PLAN_CHECK_TOPIC });
    assert.strictEqual(results.length, 0);

    ledger.close();
  });

  it('contains malformed step entries without crashing the bus', function () {
    const { ledger, messageBus } = makeBus();
    const cluster = makeCluster('pc-7');
    const detach = attachPlanChecker({ messageBus, cluster });

    const originalWarn = console.warn;
    const warnings = [];
    console.warn = (...args) => warnings.push(args.join(' '));
    try {
      publishPlan(messageBus, cluster.id, { steps: [null] });
    } finally {
      console.warn = originalWarn;
    }

    const results = messageBus.query({ cluster_id: cluster.id, topic: PLAN_CHECK_TOPIC });
    assert.strictEqual(results.length, 0);
    assert.ok(warnings.some((w) => w.includes('[plan-checks]')));

    detach();
    ledger.close();
  });

  it('requires messageBus and cluster.id', function () {
    assert.throws(() => attachPlanChecker({}), /messageBus is required/);
    assert.throws(() => attachPlanChecker({ messageBus: {} }), /cluster.id is required/);
  });
});
