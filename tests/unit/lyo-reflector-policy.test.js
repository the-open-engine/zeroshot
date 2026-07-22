const assert = require('assert');

const Ledger = require('../../src/ledger');
const MessageBus = require('../../src/message-bus');
const LessonStore = require('../../src/lyo/lesson-store');
const { attachLyoObserver } = require('../../src/lyo/observer');
const {
  TEMPLATE_REFLECTOR,
  reflectorId,
  resolveReflector,
  buildGuidanceText,
} = require('../../src/lyo/reflector-policies');

function makeBus() {
  const ledger = new Ledger(':memory:');
  const messageBus = new MessageBus(ledger);
  return { ledger, messageBus };
}

function makeCluster(clusterId, lyoConfig = { enabled: true }) {
  return {
    id: clusterId,
    config: { lyo: lyoConfig },
    agents: [{ id: 'implementer', config: { role: 'implementation' } }],
  };
}

function publishRejection(messageBus, clusterId) {
  return messageBus.publish({
    cluster_id: clusterId,
    topic: 'VALIDATION_RESULT',
    sender: 'validator',
    content: {
      text: 'Tests failed: npm test',
      data: { approved: false, errors: ['missing regression coverage'] },
    },
  });
}

function createdLessons(store) {
  return store.db.prepare('SELECT * FROM lesson').all();
}

function createDeltaPayloads(store) {
  return store.db
    .prepare("SELECT payload FROM lesson_delta WHERE delta_type = 'CREATE'")
    .all()
    .map((row) => JSON.parse(row.payload));
}

function guidanceFor(messageBus, clusterId) {
  return messageBus.queryGuidanceMailbox({
    cluster_id: clusterId,
    target_agent_id: 'implementer',
  });
}

function captureWarnings(fn) {
  const original = console.warn;
  const warnings = [];
  console.warn = (...args) => warnings.push(args.join(' '));
  try {
    fn();
  } finally {
    console.warn = original;
  }
  return warnings;
}

describe('lyo reflector policies: resolution', function () {
  it('resolves null to the template default', function () {
    assert.strictEqual(resolveReflector(null), TEMPLATE_REFLECTOR);
    assert.strictEqual(reflectorId(resolveReflector(null)), 'template@1');
  });

  it('passes object reflectors through unregistered', function () {
    const custom = { name: 'elaborator-stub', version: 1, reflect: () => ({}) };
    assert.strictEqual(resolveReflector(custom), custom);
  });

  it('resolves registry id strings and rejects unknown ones', function () {
    assert.strictEqual(resolveReflector('template@1'), TEMPLATE_REFLECTOR);
    assert.throws(() => resolveReflector('nope@1'), /unknown reflector: nope@1/);
  });

  it('template@1 reproduces the legacy guidance/explanation text', function () {
    const message = {
      content: {
        text: 'Tests failed: npm test',
        data: { errors: ['missing regression coverage'] },
      },
    };
    const reflection = TEMPLATE_REFLECTOR.reflect({ message });
    assert.match(reflection.intervention, /^Address the validator feedback before retrying\./);
    assert.match(reflection.intervention, /Tests failed: npm test/);
    assert.match(reflection.intervention, /missing regression coverage/);
    assert.match(reflection.explanation, /Tests failed: npm test/);
  });
});

describe('lyo reflector policies: observer integration', function () {
  it('defaults to template@1 and records reflector provenance in the CREATE delta', function () {
    const { ledger, messageBus } = makeBus();
    const store = new LessonStore(':memory:');
    const cluster = makeCluster('refl-1');
    const detach = attachLyoObserver({ messageBus, cluster, lessonStore: store });

    publishRejection(messageBus, cluster.id);

    const lessons = createdLessons(store);
    assert.strictEqual(lessons.length, 1);
    assert.match(lessons[0].intervention, /^Address the validator feedback before retrying\./);

    const payloads = createDeltaPayloads(store);
    assert.strictEqual(payloads.length, 1);
    assert.strictEqual(payloads[0].reflector, 'template@1');

    const guidance = guidanceFor(messageBus, cluster.id);
    assert.strictEqual(guidance.length, 1);
    assert.match(guidance[0].content.text, /Address the validator feedback/);

    detach();
    store.close();
    ledger.close();
  });

  it('uses an injected reflector for both the stored lesson and the delivered guidance', function () {
    const { ledger, messageBus } = makeBus();
    const store = new LessonStore(':memory:');
    const cluster = makeCluster('refl-2');
    let captured = null;
    const elaboratorStub = {
      name: 'elaborator-stub',
      version: 1,
      reflect({ failure_class, cue }) {
        captured = { failure_class, cue };
        return {
          explanation: 'Runs end after edits without running tests; validation then fails.',
          intervention: 'Before ending a run after edits, run the targeted tests.',
        };
      },
    };
    const detach = attachLyoObserver({
      messageBus,
      cluster,
      lessonStore: store,
      reflector: elaboratorStub,
    });

    publishRejection(messageBus, cluster.id);

    assert.ok(captured, 'reflector should have been called');
    const lessons = createdLessons(store);
    assert.strictEqual(lessons.length, 1);
    assert.strictEqual(lessons[0].failure_class, captured.failure_class);
    assert.strictEqual(
      lessons[0].intervention,
      'Before ending a run after edits, run the targeted tests.'
    );

    const payloads = createDeltaPayloads(store);
    assert.strictEqual(payloads[0].reflector, 'elaborator-stub@1');

    const guidance = guidanceFor(messageBus, cluster.id);
    assert.match(
      guidance[0].content.text,
      /^Before ending a run after edits, run the targeted tests\./
    );

    detach();
    store.close();
    ledger.close();
  });

  it('falls back to template@1 for an unknown registry id, with a warning', function () {
    const { ledger, messageBus } = makeBus();
    const store = new LessonStore(':memory:');
    const cluster = makeCluster('refl-3', { enabled: true, reflector: 'nope@1' });
    const detach = attachLyoObserver({ messageBus, cluster, lessonStore: store });

    const warnings = captureWarnings(() => publishRejection(messageBus, cluster.id));

    assert.ok(warnings.some((w) => w.includes('unknown reflector')));
    const lessons = createdLessons(store);
    assert.match(lessons[0].intervention, /^Address the validator feedback before retrying\./);
    assert.strictEqual(createDeltaPayloads(store)[0].reflector, 'template@1');

    detach();
    store.close();
    ledger.close();
  });

  it('falls back to template@1 when the reflector throws', function () {
    const { ledger, messageBus } = makeBus();
    const store = new LessonStore(':memory:');
    const cluster = makeCluster('refl-4');
    const broken = {
      name: 'broken',
      version: 1,
      reflect() {
        throw new Error('model unavailable');
      },
    };
    const detach = attachLyoObserver({
      messageBus,
      cluster,
      lessonStore: store,
      reflector: broken,
    });

    const warnings = captureWarnings(() => publishRejection(messageBus, cluster.id));

    assert.ok(warnings.some((w) => w.includes('model unavailable')));
    const guidance = guidanceFor(messageBus, cluster.id);
    assert.strictEqual(guidance.length, 1);
    assert.match(guidance[0].content.text, /Address the validator feedback/);
    assert.strictEqual(createDeltaPayloads(store)[0].reflector, 'template@1');

    detach();
    store.close();
    ledger.close();
  });

  it('falls back to template@1 when the reflector returns a malformed reflection', function () {
    const { ledger, messageBus } = makeBus();
    const store = new LessonStore(':memory:');
    const cluster = makeCluster('refl-5');
    const malformed = { name: 'malformed', version: 1, reflect: () => ({ nope: true }) };
    const detach = attachLyoObserver({
      messageBus,
      cluster,
      lessonStore: store,
      reflector: malformed,
    });

    const warnings = captureWarnings(() => publishRejection(messageBus, cluster.id));

    assert.ok(warnings.some((w) => w.includes('malformed reflection')));
    assert.strictEqual(createDeltaPayloads(store)[0].reflector, 'template@1');
    const guidance = guidanceFor(messageBus, cluster.id);
    assert.match(guidance[0].content.text, /Address the validator feedback/);

    detach();
    store.close();
    ledger.close();
  });

  it('keeps the buildGuidanceText export byte-identical for the guidance path', function () {
    const message = {
      content: {
        text: 'Tests failed: npm test',
        data: { errors: ['missing regression coverage'] },
      },
    };
    assert.strictEqual(
      buildGuidanceText(message),
      'Address the validator feedback before retrying.\n\nLatest validation:\nTests failed: npm test\n\nErrors:\n- missing regression coverage'
    );
  });
});
