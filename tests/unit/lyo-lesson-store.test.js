const assert = require('assert');

const LessonStore = require('../../src/lyo/lesson-store');
const { classifyValidationFailure } = require('../../src/lyo/failure-classifier');

// Deterministic rng (mulberry32) for reproducible Thompson draws.
function mulberry32(seed) {
  let a = seed >>> 0;
  return function () {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function makeLesson(store, overrides = {}) {
  return store.createLesson({
    failure_class: 'output_generation',
    trigger_cue: 'tests failed: npm test',
    explanation: 'Tests failed',
    intervention: 'Fix the tests',
    run_id: 'run-1',
    actor: 'reflector',
    ...overrides,
  });
}

// Record one application per outcome and immediately resolve it. Keep the
// total below the default curator watermark (25 MARK deltas) unless a test
// explicitly exercises curation.
function applyOutcomes(store, lessonId, outcomes) {
  outcomes.forEach((outcome, index) => {
    const runId = `${lessonId}-run-${index}`;
    store.recordApplication({
      lesson_id: lessonId,
      run_id: runId,
      trigger_message_id: `${runId}-msg`,
      task_cue: 'cue',
      sampled_score: 0.5,
    });
    store.applyValidationOutcome({ run_id: runId, outcome });
  });
}

describe('LYO lesson store', function () {
  it('creates the schema: tables, indexes, library view, and meta table', function () {
    const store = new LessonStore(':memory:');

    const tables = store.db
      .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
      .all()
      .map((row) => row.name);
    assert.ok(tables.includes('lesson'));
    assert.ok(tables.includes('lesson_delta'));
    assert.ok(tables.includes('lesson_application'));
    assert.ok(tables.includes('lyo_meta'));

    const views = store.db
      .prepare("SELECT name FROM sqlite_master WHERE type = 'view'")
      .all()
      .map((row) => row.name);
    assert.ok(views.includes('v_lesson_library'));

    store.close();
  });

  it('emits CREATE on first sight and EDIT-merges a duplicate cue', function () {
    const store = new LessonStore(':memory:');

    const first = store.createLesson({
      failure_class: 'output_generation',
      trigger_cue: 'Tests Failed: npm test',
      explanation: 'e1',
      intervention: 'i1',
      run_id: 'run-a',
      actor: 'reflector',
    });
    assert.strictEqual(first.status, 'candidate');
    assert.match(first.lesson_id, /^les_[0-9a-f]{16}$/);
    assert.deepStrictEqual(JSON.parse(first.provenance), ['run-a']);
    // trigger_cue is stored normalized
    assert.strictEqual(first.trigger_cue, 'tests failed: npm test');

    // Same failure_class + identical normalized cue -> merge into the existing lesson
    const second = store.createLesson({
      failure_class: 'output_generation',
      trigger_cue: 'tests   failed:  npm test',
      explanation: 'e2',
      intervention: 'i2',
      run_id: 'run-b',
      actor: 'reflector',
    });
    assert.strictEqual(second.lesson_id, first.lesson_id);
    assert.deepStrictEqual(JSON.parse(second.provenance), ['run-a', 'run-b']);
    // Text content is never rewritten by an EDIT merge (ACE no-re-summarization rule)
    assert.strictEqual(second.explanation, 'e1');
    assert.strictEqual(second.intervention, 'i1');

    const deltaTypes = store.getDeltas(first.lesson_id).map((delta) => delta.delta_type);
    assert.deepStrictEqual(deltaTypes, ['CREATE', 'EDIT']);

    // Same cue but different failure_class -> brand-new lesson
    const third = store.createLesson({
      failure_class: 'system_execution',
      trigger_cue: 'tests failed: npm test',
      explanation: 'e3',
      intervention: 'i3',
      run_id: 'run-c',
      actor: 'reflector',
    });
    assert.notStrictEqual(third.lesson_id, first.lesson_id);

    store.close();
  });

  it('records applications idempotently per (lesson, run, trigger message)', function () {
    const store = new LessonStore(':memory:');
    const lesson = makeLesson(store);

    const first = store.recordApplication({
      lesson_id: lesson.lesson_id,
      run_id: 'run-1',
      trigger_message_id: 'msg-1',
      task_cue: 'cue',
      sampled_score: 0.7,
    });
    assert.match(first.application_id, /^app_[0-9a-f]{16}$/);
    assert.strictEqual(first.outcome, 'pending');
    assert.strictEqual(first.counted, 0);
    assert.strictEqual(first.sampled_score, 0.7);

    const duplicate = store.recordApplication({
      lesson_id: lesson.lesson_id,
      run_id: 'run-1',
      trigger_message_id: 'msg-1',
      task_cue: 'cue',
      sampled_score: 0.7,
    });
    assert.strictEqual(duplicate.application_id, first.application_id);
    assert.strictEqual(store.getLesson(lesson.lesson_id).uses, 1);

    // A different trigger message is a different validation cycle -> new row
    const secondCycle = store.recordApplication({
      lesson_id: lesson.lesson_id,
      run_id: 'run-1',
      trigger_message_id: 'msg-2',
      task_cue: 'cue',
      sampled_score: 0.4,
    });
    assert.notStrictEqual(secondCycle.application_id, first.application_id);
    assert.strictEqual(store.getLesson(lesson.lesson_id).uses, 2);

    store.close();
  });

  it('moves counters only through application rows, grounded in validation outcomes', function () {
    const store = new LessonStore(':memory:');
    const lesson = makeLesson(store);

    const passApp = store.recordApplication({
      lesson_id: lesson.lesson_id,
      run_id: 'run-pass',
      trigger_message_id: 'msg-pass',
      task_cue: 'cue',
      sampled_score: 0.6,
    });
    const passResult = store.applyValidationOutcome({ run_id: 'run-pass', outcome: 'passed' });
    assert.strictEqual(passResult.updated, 1);

    let row = store.getLesson(lesson.lesson_id);
    assert.strictEqual(row.helpful_count, 1);
    assert.strictEqual(row.harmful_count, 0);

    const helpfulDelta = store
      .getDeltas(lesson.lesson_id)
      .find((delta) => delta.delta_type === 'MARK_HELPFUL');
    assert.ok(helpfulDelta);
    assert.strictEqual(helpfulDelta.actor, 'validator-rule');
    assert.deepStrictEqual(JSON.parse(helpfulDelta.payload), {
      application_id: passApp.application_id,
      outcome: 'passed',
    });

    const countedApp = store.db
      .prepare('SELECT * FROM lesson_application WHERE application_id = ?')
      .get(passApp.application_id);
    assert.strictEqual(countedApp.counted, 1);
    assert.strictEqual(countedApp.outcome, 'passed');

    // Re-applying the same run is a no-op (already counted)
    const repeatResult = store.applyValidationOutcome({ run_id: 'run-pass', outcome: 'passed' });
    assert.strictEqual(repeatResult.updated, 0);
    assert.strictEqual(store.getLesson(lesson.lesson_id).helpful_count, 1);

    store.recordApplication({
      lesson_id: lesson.lesson_id,
      run_id: 'run-fail',
      trigger_message_id: 'msg-fail',
      task_cue: 'cue',
      sampled_score: 0.6,
    });
    store.applyValidationOutcome({ run_id: 'run-fail', outcome: 'failed' });
    row = store.getLesson(lesson.lesson_id);
    assert.strictEqual(row.helpful_count, 1);
    assert.strictEqual(row.harmful_count, 1);

    // CRITICAL INVARIANT: a lesson with no application row never moves counters
    const bystander = makeLesson(store, { trigger_cue: 'lint errors in generated output' });
    store.applyValidationOutcome({ run_id: 'run-unrelated', outcome: 'passed' });
    store.applyValidationOutcome({ run_id: 'run-pass', outcome: 'failed' });
    const bystanderRow = store.getLesson(bystander.lesson_id);
    assert.strictEqual(bystanderRow.helpful_count, 0);
    assert.strictEqual(bystanderRow.harmful_count, 0);

    store.close();
  });

  it('promotes a candidate to active after 8 helpful outcomes (no PROMOTE delta)', function () {
    const store = new LessonStore(':memory:');
    const lesson = makeLesson(store);

    applyOutcomes(store, lesson.lesson_id, Array(8).fill('passed'));

    const row = store.getLesson(lesson.lesson_id);
    assert.strictEqual(row.helpful_count, 8);
    assert.strictEqual(row.harmful_count, 0);
    assert.strictEqual(row.status, 'active');

    // Documented deviation: promotion folds into the row WITHOUT a delta;
    // only CREATE + 8 MARK_HELPFUL deltas exist.
    const deltaTypes = store.getDeltas(lesson.lesson_id).map((delta) => delta.delta_type);
    assert.deepStrictEqual(deltaTypes, ['CREATE', ...Array(8).fill('MARK_HELPFUL')]);

    store.close();
  });

  it('quarantines a lesson when the Wilson upper bound drops below useful', function () {
    const store = new LessonStore(':memory:');
    const lesson = makeLesson(store, { trigger_cue: 'build broke on ci' });

    // At n = 8 (1 passed + 7 failed) the Wilson upper bound is ~0.471 > 0.45:
    // still a candidate, not yet quarantined.
    applyOutcomes(store, lesson.lesson_id, ['passed', ...Array(7).fill('failed')]);
    assert.strictEqual(store.getLesson(lesson.lesson_id).status, 'candidate');

    // At n = 9 (1 passed + 8 failed) the upper bound is ~0.435 < 0.45: quarantine.
    const runId = `${lesson.lesson_id}-run-final`;
    store.recordApplication({
      lesson_id: lesson.lesson_id,
      run_id: runId,
      trigger_message_id: `${runId}-msg`,
      task_cue: 'cue',
      sampled_score: 0.5,
    });
    store.applyValidationOutcome({ run_id: runId, outcome: 'failed' });

    const row = store.getLesson(lesson.lesson_id);
    assert.strictEqual(row.helpful_count, 1);
    assert.strictEqual(row.harmful_count, 8);
    assert.strictEqual(row.status, 'quarantined');

    const quarantineDelta = store
      .getDeltas(lesson.lesson_id)
      .find((delta) => delta.delta_type === 'QUARANTINE');
    assert.ok(quarantineDelta);
    assert.strictEqual(quarantineDelta.actor, 'validator-rule');

    // Quarantined lessons leave the library view but keep their rows
    const viewRows = store.db
      .prepare('SELECT * FROM v_lesson_library WHERE lesson_id = ?')
      .all(lesson.lesson_id);
    assert.strictEqual(viewRows.length, 0);

    // Replay reconstructs the same state from deltas alone
    const replayed = store.replayLesson(lesson.lesson_id);
    assert.strictEqual(replayed.status, 'quarantined');
    assert.strictEqual(replayed.helpful_count, 1);
    assert.strictEqual(replayed.harmful_count, 8);

    store.close();
  });

  it('selectLessons Thompson-samples the library view', function () {
    const store = new LessonStore(':memory:');
    makeLesson(store, { trigger_cue: 'cue alpha' });
    makeLesson(store, { trigger_cue: 'cue beta' });
    makeLesson(store, { trigger_cue: 'cue gamma' });
    makeLesson(store, { failure_class: 'system_execution', trigger_cue: 'cue other class' });

    const selected = store.selectLessons({ failure_class: 'output_generation', limit: 2 });
    assert.strictEqual(selected.length, 2);
    for (const row of selected) {
      assert.strictEqual(row.failure_class, 'output_generation');
      assert.ok(row.sampled_score >= 0 && row.sampled_score <= 1);
    }

    // Candidate-status lessons are part of the view (exploration)
    const all = store.selectLessons({ failure_class: 'output_generation', limit: 10 });
    assert.strictEqual(all.length, 3);

    // An injected deterministic rng makes the draw reproducible
    const first = store.selectLessons({
      failure_class: 'output_generation',
      limit: 3,
      rng: mulberry32(42),
    });
    const second = store.selectLessons({
      failure_class: 'output_generation',
      limit: 3,
      rng: mulberry32(42),
    });
    assert.deepStrictEqual(
      first.map((row) => [row.lesson_id, row.sampled_score]),
      second.map((row) => [row.lesson_id, row.sampled_score])
    );

    store.close();
  });

  it('maybeCurate does nothing below the MARK-delta watermark', function () {
    const store = new LessonStore(':memory:');
    const stale = makeLesson(store, { trigger_cue: 'stale candidate cue' });
    const oldIso = new Date(Date.now() - 60 * 24 * 60 * 60 * 1000).toISOString();
    store.db
      .prepare('UPDATE lesson SET created_at = ? WHERE lesson_id = ?')
      .run(oldIso, stale.lesson_id);

    const driver = makeLesson(store, { trigger_cue: 'evidence cue' });
    applyOutcomes(store, driver.lesson_id, ['passed', 'failed']); // 2 MARK deltas < 25

    const result = store.maybeCurate();
    assert.strictEqual(result.curated, false);
    assert.strictEqual(store.getLesson(stale.lesson_id).status, 'candidate');

    store.close();
  });

  it('maybeCurate prunes stale candidates and merges duplicate cues above the watermark', function () {
    const store = new LessonStore(':memory:');

    const driver = makeLesson(store, { trigger_cue: 'driver cue' });
    applyOutcomes(store, driver.lesson_id, ['passed', 'passed', 'failed']); // 3 MARK deltas

    // Stale candidate: uses = 0, created long ago
    const stale = makeLesson(store, { trigger_cue: 'stale candidate cue' });
    const oldIso = new Date(Date.now() - 45 * 24 * 60 * 60 * 1000).toISOString();
    store.db
      .prepare('UPDATE lesson SET created_at = ? WHERE lesson_id = ?')
      .run(oldIso, stale.lesson_id);

    // Exact-duplicate cue pair, inserted directly (createLesson would merge them)
    const now = new Date().toISOString();
    const insertLesson = store.db.prepare(
      `INSERT INTO lesson (
         lesson_id, status, failure_class, trigger_cue, explanation, intervention,
         helpful_count, harmful_count, uses, created_at, updated_at, provenance
       ) VALUES (?, 'candidate', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`
    );
    const insertDelta = store.db.prepare(
      `INSERT INTO lesson_delta (lesson_id, run_id, actor, delta_type, payload)
       VALUES (?, NULL, 'reflector', 'CREATE', ?)`
    );
    const createPayload = (explanation, intervention, provenance) =>
      JSON.stringify({
        failure_class: 'output_generation',
        trigger_cue: 'duplicated cue',
        explanation,
        intervention,
        created_at: now,
        updated_at: now,
        provenance,
      });
    insertLesson.run(
      'les_dup_target',
      'output_generation',
      'duplicated cue',
      'e-target',
      'i-target',
      3,
      0,
      3,
      now,
      now,
      JSON.stringify(['run-t'])
    );
    insertDelta.run('les_dup_target', createPayload('e-target', 'i-target', ['run-t']));
    insertLesson.run(
      'les_dup_source',
      'output_generation',
      'duplicated cue',
      'e-source',
      'i-source',
      1,
      1,
      5,
      now,
      now,
      JSON.stringify(['run-s', 'run-t2'])
    );
    insertDelta.run('les_dup_source', createPayload('e-source', 'i-source', ['run-s', 'run-t2']));
    // Counters only move via MARK_* deltas, so the fabricated rows need a
    // delta-consistent counter history for replay to reconstruct them.
    const insertMarkDelta = store.db.prepare(
      `INSERT INTO lesson_delta (lesson_id, run_id, actor, delta_type, payload)
       VALUES (?, NULL, 'validator-rule', ?, ?)`
    );
    for (let i = 0; i < 3; i++) {
      insertMarkDelta.run(
        'les_dup_target',
        'MARK_HELPFUL',
        JSON.stringify({ application_id: `app_t${i}`, outcome: 'passed' })
      );
    }
    insertMarkDelta.run(
      'les_dup_source',
      'MARK_HELPFUL',
      JSON.stringify({ application_id: 'app_s0', outcome: 'passed' })
    );
    insertMarkDelta.run(
      'les_dup_source',
      'MARK_HARMFUL',
      JSON.stringify({ application_id: 'app_s1', outcome: 'failed' })
    );

    const result = store.maybeCurate({ markInterval: 3 });
    assert.strictEqual(result.curated, true);
    assert.strictEqual(result.merged, 1);
    assert.strictEqual(result.pruned, 1);

    // Prune: stale candidate retired via a RETIRE delta (never hard-deleted)
    const staleRow = store.getLesson(stale.lesson_id);
    assert.strictEqual(staleRow.status, 'retired');
    const retireDelta = store
      .getDeltas(stale.lesson_id)
      .find((delta) => delta.delta_type === 'RETIRE');
    assert.ok(retireDelta);
    assert.strictEqual(retireDelta.actor, 'curator');

    // Merge: highest (helpful + harmful) row absorbs the other
    const target = store.getLesson('les_dup_target');
    assert.strictEqual(target.helpful_count, 4);
    assert.strictEqual(target.harmful_count, 1);
    assert.strictEqual(target.uses, 8);
    assert.deepStrictEqual(JSON.parse(target.provenance), ['run-t', 'run-s', 'run-t2']);
    // Curation never rewrites lesson text (ACE no-re-summarization rule)
    assert.strictEqual(target.explanation, 'e-target');
    assert.strictEqual(target.intervention, 'i-target');
    assert.strictEqual(target.trigger_cue, 'duplicated cue');

    const source = store.getLesson('les_dup_source');
    assert.strictEqual(source.status, 'retired');
    assert.strictEqual(source.helpful_count, 0);
    assert.strictEqual(source.harmful_count, 0);
    const mergeDelta = store
      .getDeltas('les_dup_source')
      .find((delta) => delta.delta_type === 'MERGE_INTO');
    assert.ok(mergeDelta);
    assert.strictEqual(mergeDelta.actor, 'curator');
    assert.strictEqual(JSON.parse(mergeDelta.payload).target_lesson_id, 'les_dup_target');

    // Replay reconstructs both sides of the merge from deltas
    const targetReplay = store.replayLesson('les_dup_target');
    assert.strictEqual(targetReplay.helpful_count, 4);
    assert.strictEqual(targetReplay.harmful_count, 1);
    assert.deepStrictEqual(targetReplay.provenance, ['run-t', 'run-s', 'run-t2']);
    const sourceReplay = store.replayLesson('les_dup_source');
    assert.strictEqual(sourceReplay.status, 'retired');
    assert.strictEqual(sourceReplay.helpful_count, 0);

    store.close();
  });

  it('replayLesson folds deltas back to the current lesson row state', function () {
    const store = new LessonStore(':memory:');
    const lesson = makeLesson(store, {
      trigger_cue: 'replay cue',
      run_id: 'replay-a',
      explanation: 'original explanation',
      intervention: 'original intervention',
    });
    // EDIT merge from a second run
    makeLesson(store, { trigger_cue: 'replay cue', run_id: 'replay-b' });
    // 7 passed + 1 failed -> n = 8, wilson_lower > 0.5 -> active
    applyOutcomes(store, lesson.lesson_id, [
      'passed',
      'passed',
      'failed',
      'passed',
      'passed',
      'passed',
      'passed',
      'passed',
    ]);

    const row = store.getLesson(lesson.lesson_id);
    const replayed = store.replayLesson(lesson.lesson_id);

    assert.strictEqual(replayed.lesson_id, row.lesson_id);
    assert.strictEqual(replayed.status, row.status);
    assert.strictEqual(replayed.status, 'active');
    assert.strictEqual(replayed.helpful_count, row.helpful_count);
    assert.strictEqual(replayed.harmful_count, row.harmful_count);
    assert.deepStrictEqual(replayed.provenance, JSON.parse(row.provenance));
    assert.deepStrictEqual(replayed.provenance, ['replay-a', 'replay-b']);
    assert.strictEqual(replayed.explanation, row.explanation);
    assert.strictEqual(replayed.intervention, row.intervention);
    assert.strictEqual(replayed.trigger_cue, row.trigger_cue);
    assert.strictEqual(replayed.created_at, row.created_at);
    assert.strictEqual(replayed.updated_at, row.updated_at);

    store.close();
  });
});

describe('LYO failure classifier', function () {
  it('maps validator feedback to the seeded taxonomy deterministically', function () {
    const asMessage = (text, errors, criteriaResults) => ({
      content: { text, data: { errors, criteriaResults } },
    });

    assert.strictEqual(
      classifyValidationFailure(asMessage('Tests failed: npm test', ['missing coverage']))
        .failure_class,
      'output_generation'
    );
    assert.strictEqual(
      classifyValidationFailure(asMessage('git push failed: permission denied')).failure_class,
      'system_execution'
    );
    assert.strictEqual(
      classifyValidationFailure(asMessage('agent stuck in a loop during handoff')).failure_class,
      'orchestration'
    );
    assert.strictEqual(
      classifyValidationFailure(asMessage('context truncated: token limit exceeded')).failure_class,
      'context_handling'
    );
    assert.strictEqual(
      classifyValidationFailure(asMessage('wrong tool used: api misuse of parameter'))
        .failure_class,
      'tool_selection'
    );
    assert.strictEqual(
      classifyValidationFailure(asMessage('requirement misunderstood, goal not met')).failure_class,
      'goal_deviation'
    );
    // Fallback
    assert.strictEqual(
      classifyValidationFailure(asMessage('rejected without a recognized cause')).failure_class,
      'output_generation'
    );
  });

  it('builds the cue from the first error line, normalized and truncated', function () {
    const { cue } = classifyValidationFailure({
      content: {
        text: 'IGNORED TEXT',
        data: { errors: ['  Missing   Regression\nsecond line'], approved: false },
      },
    });
    assert.strictEqual(cue, 'missing regression');

    const textCue = classifyValidationFailure({
      content: { text: 'Some Failure Happened\ndetails', data: { approved: false } },
    });
    assert.strictEqual(textCue.cue, 'some failure happened');

    const longCue = classifyValidationFailure({
      content: { text: 'x'.repeat(500), data: { approved: false } },
    });
    assert.ok(longCue.cue.length <= 120);
  });
});
