const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');

const Ledger = require('../../src/ledger');
const MessageBus = require('../../src/message-bus');
const { USER_GUIDANCE_AGENT } = require('../../src/guidance-topics');
const { attachLyoObserver } = require('../../src/lyo/observer');
const LessonStore = require('../../src/lyo/lesson-store');
const { createTestOrchestrator } = require('../helpers/setup');

describe('LYO observer', function () {
  it('turns rejected validation results into queued implementation guidance', function () {
    const ledger = new Ledger(':memory:');
    const messageBus = new MessageBus(ledger);
    const clusterId = 'lyo-observer-1';
    const cluster = {
      id: clusterId,
      config: { lyo: { enabled: true } },
      agents: [
        { id: 'implementer', config: { role: 'implementation' } },
        { id: 'validator', config: { role: 'validation' } },
      ],
    };

    // Inject an in-memory store: without one the observer falls through to
    // the real global ~/.zeroshot/lyo-lessons.db and pollutes it.
    const store = new LessonStore(':memory:');
    const detach = attachLyoObserver({ messageBus, cluster, lessonStore: store });

    const validation = messageBus.publish({
      cluster_id: clusterId,
      topic: 'VALIDATION_RESULT',
      sender: 'validator',
      content: {
        text: 'Tests failed: npm test',
        data: {
          approved: false,
          errors: ['missing regression coverage'],
        },
      },
    });

    const interventions = messageBus.query({ cluster_id: clusterId, topic: 'LYO_INTERVENTION' });
    assert.strictEqual(interventions.length, 1);
    assert.strictEqual(interventions[0].sender, 'lyo');
    assert.strictEqual(interventions[0].content.data.trigger_message_id, validation.id);
    assert.match(interventions[0].content.text, /validation rejected/i);

    const guidance = messageBus.queryGuidanceMailbox({
      cluster_id: clusterId,
      target_agent_id: 'implementer',
    });
    assert.strictEqual(guidance.length, 1);
    assert.strictEqual(guidance[0].topic, USER_GUIDANCE_AGENT);
    assert.strictEqual(guidance[0].sender, 'lyo');
    assert.strictEqual(guidance[0].receiver, 'implementer');
    assert.match(guidance[0].content.text, /Address the validator feedback/i);
    assert.match(guidance[0].content.text, /Tests failed: npm test/);

    detach();
    store.close();
    ledger.close();
  });

  it('records feedback outcome for the next validation after an intervention', function () {
    const ledger = new Ledger(':memory:');
    const messageBus = new MessageBus(ledger);
    const clusterId = 'lyo-feedback-1';
    const cluster = {
      id: clusterId,
      config: { lyo: { enabled: true } },
      agents: [{ id: 'worker', config: { role: 'implementation' } }],
    };

    const store = new LessonStore(':memory:');
    const detach = attachLyoObserver({ messageBus, cluster, lessonStore: store });

    messageBus.publish({
      cluster_id: clusterId,
      topic: 'VALIDATION_RESULT',
      sender: 'validator',
      content: {
        text: 'Need regression coverage',
        data: { approved: false },
      },
    });
    const intervention = messageBus.findLast({ cluster_id: clusterId, topic: 'LYO_INTERVENTION' });

    const approval = messageBus.publish({
      cluster_id: clusterId,
      topic: 'VALIDATION_RESULT',
      sender: 'validator',
      content: {
        text: 'Accepted after adding coverage',
        data: { approved: true },
      },
    });

    const feedback = messageBus.query({ cluster_id: clusterId, topic: 'LYO_FEEDBACK' });
    assert.strictEqual(feedback.length, 1);
    assert.strictEqual(feedback[0].sender, 'lyo');
    assert.strictEqual(feedback[0].content.data.intervention_id, intervention.id);
    assert.strictEqual(feedback[0].content.data.feedback_message_id, approval.id);
    assert.match(feedback[0].content.text, /accepted/);

    const guidance = messageBus.queryGuidanceMailbox({
      cluster_id: clusterId,
      target_agent_id: 'worker',
    });
    assert.strictEqual(guidance.length, 1);

    detach();
    store.close();
    ledger.close();
  });
});

describe('LYO start option', function () {
  it('can be enabled by a start option without changing the config file', async function () {
    const { orchestrator, cleanup } = createTestOrchestrator();
    const config = {
      agents: [
        {
          id: 'worker',
          role: 'implementation',
          modelLevel: 'level1',
          triggers: [],
        },
      ],
    };

    try {
      const result = await orchestrator.start(
        config,
        { text: 'Implement with learning' },
        { testMode: true, lyo: true }
      );
      const cluster = orchestrator.getCluster(result.id);

      cluster.messageBus.publish({
        cluster_id: result.id,
        topic: 'VALIDATION_RESULT',
        sender: 'validator',
        content: {
          text: 'Rejected so LYO can advise',
          data: { approved: false },
        },
      });

      const guidance = cluster.messageBus.queryGuidanceMailbox({
        cluster_id: result.id,
        target_agent_id: 'worker',
      });
      assert.strictEqual(guidance.length, 1);
      assert.strictEqual(guidance[0].sender, 'lyo');
    } finally {
      await cleanup();
    }
  });
});

describe('LYO observer lesson learning', function () {
  function createCluster(clusterId) {
    return {
      id: clusterId,
      config: { lyo: { enabled: true } },
      agents: [{ id: 'worker', config: { role: 'implementation' } }],
    };
  }

  function publishValidation(messageBus, clusterId, { approved, text, errors }) {
    return messageBus.publish({
      cluster_id: clusterId,
      topic: 'VALIDATION_RESULT',
      sender: 'validator',
      content: {
        text,
        data: { approved, errors },
      },
    });
  }

  it('creates a candidate lesson, records applications, and annotates guidance on rejection', function () {
    const ledger = new Ledger(':memory:');
    const messageBus = new MessageBus(ledger);
    const store = new LessonStore(':memory:');
    const clusterId = 'lyo-learn-1';
    const cluster = createCluster(clusterId);

    const detach = attachLyoObserver({ messageBus, cluster, lessonStore: store });

    const validation = publishValidation(messageBus, clusterId, {
      approved: false,
      text: 'Tests failed: npm test',
      errors: ['missing regression coverage'],
    });

    const lessons = store.db.prepare('SELECT * FROM lesson').all();
    assert.strictEqual(lessons.length, 1);
    assert.strictEqual(lessons[0].status, 'candidate');
    assert.strictEqual(lessons[0].failure_class, 'output_generation');
    assert.strictEqual(lessons[0].trigger_cue, 'missing regression coverage');

    const applications = store.db.prepare('SELECT * FROM lesson_application').all();
    assert.strictEqual(applications.length, 1);
    assert.strictEqual(applications[0].lesson_id, lessons[0].lesson_id);
    assert.strictEqual(applications[0].run_id, clusterId);
    assert.strictEqual(applications[0].trigger_message_id, validation.id);
    assert.strictEqual(applications[0].outcome, 'pending');
    assert.strictEqual(applications[0].counted, 0);

    const guidance = messageBus.queryGuidanceMailbox({
      cluster_id: clusterId,
      target_agent_id: 'worker',
    });
    assert.strictEqual(guidance.length, 1);
    // Existing guidance prefix stays verbatim; the lesson section comes after it
    assert.match(guidance[0].content.text, /^Address the validator feedback before retrying\./);
    assert.match(guidance[0].content.text, /Lessons from past failures \(LYO\):/);
    assert.ok(guidance[0].content.text.includes(`[${lessons[0].lesson_id}]`));
    assert.ok(Array.isArray(guidance[0].content.data.lessons));
    assert.strictEqual(guidance[0].content.data.lessons.length, 1);
    assert.strictEqual(guidance[0].content.data.lessons[0].lesson_id, lessons[0].lesson_id);
    assert.strictEqual(
      guidance[0].content.data.lessons[0].application_id,
      applications[0].application_id
    );
    assert.strictEqual(guidance[0].content.data.lessons[0].failure_class, 'output_generation');

    const interventions = messageBus.query({ cluster_id: clusterId, topic: 'LYO_INTERVENTION' });
    assert.strictEqual(interventions.length, 1);
    assert.ok(Array.isArray(interventions[0].content.data.lessons));
    assert.strictEqual(interventions[0].content.data.lessons[0].lesson_id, lessons[0].lesson_id);

    detach();
    // An injected store is owned by the caller: detach must NOT close it
    assert.ok(store.db.prepare('SELECT 1 AS ok').get().ok);
    store.close();
    ledger.close();
  });

  it('counts an approved follow-up validation as helpful', function () {
    const ledger = new Ledger(':memory:');
    const messageBus = new MessageBus(ledger);
    const store = new LessonStore(':memory:');
    const clusterId = 'lyo-learn-2';
    const cluster = createCluster(clusterId);

    const detach = attachLyoObserver({ messageBus, cluster, lessonStore: store });

    publishValidation(messageBus, clusterId, {
      approved: false,
      text: 'Tests failed: npm test',
      errors: ['missing regression coverage'],
    });
    publishValidation(messageBus, clusterId, {
      approved: true,
      text: 'Accepted after adding coverage',
    });

    const lesson = store.db.prepare('SELECT * FROM lesson').get();
    assert.strictEqual(lesson.helpful_count, 1);
    assert.strictEqual(lesson.harmful_count, 0);

    const application = store.db.prepare('SELECT * FROM lesson_application').get();
    assert.strictEqual(application.outcome, 'passed');
    assert.strictEqual(application.counted, 1);

    const helpfulDelta = store
      .getDeltas(lesson.lesson_id)
      .find((delta) => delta.delta_type === 'MARK_HELPFUL');
    assert.ok(helpfulDelta);

    // The pre-existing feedback message behavior is unchanged
    const feedback = messageBus.query({ cluster_id: clusterId, topic: 'LYO_FEEDBACK' });
    assert.strictEqual(feedback.length, 1);
    assert.match(feedback[0].content.text, /accepted/);

    detach();
    store.close();
    ledger.close();
  });

  it('counts a rejected follow-up validation as harmful and starts a new cycle', function () {
    const ledger = new Ledger(':memory:');
    const messageBus = new MessageBus(ledger);
    const store = new LessonStore(':memory:');
    const clusterId = 'lyo-learn-3';
    const cluster = createCluster(clusterId);

    const detach = attachLyoObserver({ messageBus, cluster, lessonStore: store });

    publishValidation(messageBus, clusterId, {
      approved: false,
      text: 'Tests failed: npm test',
      errors: ['missing regression coverage'],
    });
    publishValidation(messageBus, clusterId, {
      approved: false,
      text: 'Tests failed: npm test',
      errors: ['missing regression coverage'],
    });

    // Same cue: the second rejection EDIT-merges into the same lesson
    const lessons = store.db.prepare('SELECT * FROM lesson').all();
    assert.strictEqual(lessons.length, 1);
    assert.strictEqual(lessons[0].helpful_count, 0);
    assert.strictEqual(lessons[0].harmful_count, 1);

    const applications = store.db.prepare('SELECT * FROM lesson_application ORDER BY rowid').all();
    assert.strictEqual(applications.length, 2);
    assert.strictEqual(applications[0].outcome, 'failed');
    assert.strictEqual(applications[0].counted, 1);
    // The second rejection already opened the next attribution cycle
    assert.strictEqual(applications[1].outcome, 'pending');
    assert.strictEqual(applications[1].counted, 0);

    detach();
    store.close();
    ledger.close();
  });

  it('runs degraded (guidance without lessons) when the store cannot be opened', function () {
    const ledger = new Ledger(':memory:');
    const messageBus = new MessageBus(ledger);
    const clusterId = 'lyo-degraded-1';

    // A store path underneath a regular file: mkdir/open must fail
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'lyo-degraded-'));
    const blockerFile = path.join(tempDir, 'blocker');
    fs.writeFileSync(blockerFile, 'not a directory');
    const impossibleStorePath = path.join(blockerFile, 'nested', 'lyo-lessons.db');

    const cluster = {
      id: clusterId,
      config: { lyo: { enabled: true, storePath: impossibleStorePath } },
      agents: [{ id: 'worker', config: { role: 'implementation' } }],
    };

    const warnings = [];
    const originalWarn = console.warn;
    console.warn = (...args) => warnings.push(args.join(' '));

    let detach;
    try {
      detach = attachLyoObserver({ messageBus, cluster, lessonStore: null });

      publishValidation(messageBus, clusterId, {
        approved: false,
        text: 'Tests failed: npm test',
        errors: ['missing regression coverage'],
      });
    } finally {
      console.warn = originalWarn;
    }

    assert.ok(
      warnings.some((warning) => warning.includes('[lyo] lesson store unavailable')),
      'expected a degraded-mode warning'
    );

    const guidance = messageBus.queryGuidanceMailbox({
      cluster_id: clusterId,
      target_agent_id: 'worker',
    });
    assert.strictEqual(guidance.length, 1);
    assert.match(guidance[0].content.text, /Address the validator feedback/i);
    assert.ok(!guidance[0].content.text.includes('Lessons from past failures'));
    assert.strictEqual(guidance[0].content.data.lessons, null);

    const interventions = messageBus.query({ cluster_id: clusterId, topic: 'LYO_INTERVENTION' });
    assert.strictEqual(interventions.length, 1);
    assert.strictEqual(interventions[0].content.data.lessons, null);

    detach();
    ledger.close();
    fs.rmSync(tempDir, { recursive: true, force: true });
  });

  it('logs one decision row per rejection and applications carry decision_id', function () {
    const ledger = new Ledger(':memory:');
    const messageBus = new MessageBus(ledger);
    const store = new LessonStore(':memory:');
    const clusterId = 'lyo-decision-1';
    const cluster = createCluster(clusterId);

    const detach = attachLyoObserver({ messageBus, cluster, lessonStore: store });
    const validation = publishValidation(messageBus, clusterId, {
      approved: false,
      text: 'Tests failed: npm test',
      errors: ['missing regression coverage'],
    });

    const decisions = store.db.prepare('SELECT * FROM lesson_decision').all();
    assert.strictEqual(decisions.length, 1);
    const decision = decisions[0];
    assert.strictEqual(decision.run_id, clusterId);
    assert.strictEqual(decision.trigger_message_id, validation.id);
    assert.strictEqual(decision.cycle_index, 1);
    assert.strictEqual(decision.failure_class, 'output_generation');
    assert.strictEqual(decision.null_arm, 0);
    assert.strictEqual(decision.policy, 'thompson-beta@1');

    const candidates = JSON.parse(decision.candidates);
    assert.strictEqual(candidates.length, 1);
    assert.strictEqual(candidates[0].alpha, 1);
    assert.strictEqual(candidates[0].beta, 1);
    assert.strictEqual(candidates[0].propensity, 1);
    const selected = JSON.parse(decision.selected);
    assert.strictEqual(selected.length, 1);
    assert.strictEqual(selected[0].lesson_id, candidates[0].lesson_id);
    assert.ok(selected[0].score >= 0 && selected[0].score <= 1);

    const application = store.db.prepare('SELECT * FROM lesson_application').get();
    assert.strictEqual(application.decision_id, decision.decision_id);

    const guidance = messageBus.queryGuidanceMailbox({
      cluster_id: clusterId,
      target_agent_id: 'worker',
    });
    assert.strictEqual(guidance[0].content.data.lessons[0].decision_id, decision.decision_id);
    assert.strictEqual(guidance[0].content.data.lessons[0].propensity, 1);

    detach();
    store.close();
    ledger.close();
  });

  it('increments cycle_index across cycles while counter rules still apply', function () {
    const ledger = new Ledger(':memory:');
    const messageBus = new MessageBus(ledger);
    const store = new LessonStore(':memory:');
    const clusterId = 'lyo-decision-2';
    const cluster = createCluster(clusterId);

    const detach = attachLyoObserver({ messageBus, cluster, lessonStore: store });

    publishValidation(messageBus, clusterId, {
      approved: false,
      text: 'Tests failed: npm test',
      errors: ['missing regression coverage'],
    });
    // Resolves cycle 1 (failed), opens cycle 2.
    publishValidation(messageBus, clusterId, {
      approved: false,
      text: 'Tests failed: npm test',
      errors: ['missing regression coverage'],
    });

    const decisions = store.db.prepare('SELECT * FROM lesson_decision ORDER BY rowid').all();
    assert.strictEqual(decisions.length, 2);
    assert.strictEqual(decisions[0].cycle_index, 1);
    assert.strictEqual(decisions[1].cycle_index, 2);

    // Counter rule untouched: the first application resolved failed/counted,
    // the second cycle's application is pending — and both join to decisions.
    const applications = store.db.prepare('SELECT * FROM lesson_application ORDER BY rowid').all();
    assert.strictEqual(applications.length, 2);
    assert.strictEqual(applications[0].outcome, 'failed');
    assert.strictEqual(applications[0].counted, 1);
    assert.strictEqual(applications[0].decision_id, decisions[0].decision_id);
    assert.strictEqual(applications[1].outcome, 'pending');
    assert.strictEqual(applications[1].decision_id, decisions[1].decision_id);

    detach();
    store.close();
    ledger.close();
  });

  it('uses an injected selection policy and records its id in the decision', function () {
    const ledger = new Ledger(':memory:');
    const messageBus = new MessageBus(ledger);
    const store = new LessonStore(':memory:');
    const clusterId = 'lyo-policy-1';
    const cluster = createCluster(clusterId);

    // Deterministic no-op policy: picks the first `limit` candidates as-is.
    const echoPolicy = {
      name: 'echo',
      version: 2,
      sampleSelection(candidates, limit) {
        return candidates
          .map((candidate, index) => ({ index, score: null }))
          .slice(0, Math.max(0, limit));
      },
    };

    const detach = attachLyoObserver({
      messageBus,
      cluster,
      lessonStore: store,
      selectionPolicy: echoPolicy,
    });
    publishValidation(messageBus, clusterId, {
      approved: false,
      text: 'Tests failed: npm test',
      errors: ['missing regression coverage'],
    });

    const decision = store.db.prepare('SELECT * FROM lesson_decision').get();
    assert.strictEqual(decision.policy, 'echo@2');

    detach();
    store.close();
    ledger.close();
  });
});
