const assert = require('assert');

const LessonStore = require('../../src/lyo/lesson-store');

function makeTraces(store) {
  const chosen = store.recordTrace({
    trace_id: 'trace-chosen-implementation',
    run_id: 'run-chosen',
    kind: 'behavior',
    summary: 'Preferred implementation strategy.',
    ref: 'evidence-chosen',
  });
  const rejected = store.recordTrace({
    trace_id: 'trace-rejected-implementation',
    run_id: 'run-rejected',
    kind: 'behavior',
    summary: 'Rejected complex solution.',
    ref: 'evidence-rejected',
  });
  return { chosen, rejected };
}

describe('LYO preference pairs', function () {
  it('creates the learning_trace and preference_pair tables', function () {
    const store = new LessonStore(':memory:');

    const tables = store.db
      .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
      .all()
      .map((row) => row.name);
    assert.ok(tables.includes('learning_trace'));
    assert.ok(tables.includes('preference_pair'));

    store.close();
  });

  it('derives a deterministic trace id from content when none is given', function () {
    const store = new LessonStore(':memory:');

    const trace = store.recordTrace({
      run_id: 'run-1',
      kind: 'tool_use',
      summary: 'Ran npm test.',
    });
    assert.ok(trace.trace_id.startsWith('trace-'));
    assert.strictEqual(trace.trace_id.length, 'trace-'.length + 24);
    assert.strictEqual(trace.run_id, 'run-1');
    assert.strictEqual(trace.kind, 'tool_use');

    store.close();
  });

  it('records a preference pair over distinct traces', function () {
    const store = new LessonStore(':memory:');
    const { chosen, rejected } = makeTraces(store);

    const pair = store.recordPreferencePair({
      chosen_trace_id: chosen.trace_id,
      rejected_trace_id: rejected.trace_id,
      reason: 'Prefer simple local CLI player over complex Telegram bot daemon.',
      evidence_ref: 'github-issue-12',
      confidence: 'high',
      recorded_by: 'user-preference-test',
    });

    assert.ok(pair.preference_id.startsWith('pref-'));
    assert.strictEqual(pair.chosen_trace_id, chosen.trace_id);
    assert.strictEqual(pair.rejected_trace_id, rejected.trace_id);
    assert.strictEqual(pair.confidence, 'high');
    assert.strictEqual(pair.recorded_by, 'user-preference-test');
    // Default context hash is derived from the ordered pair.
    assert.strictEqual(pair.context_hash.length, 64);

    store.close();
  });

  it('defaults confidence to medium', function () {
    const store = new LessonStore(':memory:');
    const { chosen, rejected } = makeTraces(store);

    const pair = store.recordPreferencePair({
      chosen_trace_id: chosen.trace_id,
      rejected_trace_id: rejected.trace_id,
      reason: 'Chosen trace matches the requested scope better.',
      evidence_ref: 'evidence-1',
    });
    assert.strictEqual(pair.confidence, 'medium');

    store.close();
  });

  it('rejects identical chosen and rejected traces', function () {
    const store = new LessonStore(':memory:');
    const { chosen } = makeTraces(store);

    assert.throws(
      () =>
        store.recordPreferencePair({
          chosen_trace_id: chosen.trace_id,
          rejected_trace_id: chosen.trace_id,
          reason: 'This reason is long enough to pass.',
          evidence_ref: 'evidence-1',
        }),
      /preference pair requires distinct chosen and rejected traces/
    );

    store.close();
  });

  it('rejects vague reasons that cannot be audited', function () {
    const store = new LessonStore(':memory:');
    const { chosen, rejected } = makeTraces(store);

    assert.throws(
      () =>
        store.recordPreferencePair({
          chosen_trace_id: chosen.trace_id,
          rejected_trace_id: rejected.trace_id,
          reason: 'better',
          evidence_ref: 'evidence-1',
        }),
      /preference reason must be specific enough to audit/
    );

    store.close();
  });

  it('rejects pairs referencing unknown traces', function () {
    const store = new LessonStore(':memory:');
    const { chosen } = makeTraces(store);

    assert.throws(
      () =>
        store.recordPreferencePair({
          chosen_trace_id: chosen.trace_id,
          rejected_trace_id: 'trace-does-not-exist',
          reason: 'This reason is long enough to pass.',
          evidence_ref: 'evidence-1',
        }),
      /unknown trace: trace-does-not-exist/
    );

    store.close();
  });

  it('rejects an invalid confidence value', function () {
    const store = new LessonStore(':memory:');
    const { chosen, rejected } = makeTraces(store);

    assert.throws(() =>
      store.recordPreferencePair({
        chosen_trace_id: chosen.trace_id,
        rejected_trace_id: rejected.trace_id,
        reason: 'This reason is long enough to pass.',
        evidence_ref: 'evidence-1',
        confidence: 'very-high',
      })
    );

    store.close();
  });
});
