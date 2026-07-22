const assert = require('assert');

const Ledger = require('../../src/ledger');
const MessageBus = require('../../src/message-bus');
const { USER_GUIDANCE_AGENT } = require('../../src/guidance-topics');
const { attachLyoObserver } = require('../../src/lyo/observer');
const LessonStore = require('../../src/lyo/lesson-store');
const { resolveReflector } = require('../../src/lyo/reflector-policies');
const {
  createElaboratorReflector,
  buildPrompt,
  parseReflectionJson,
  DEFAULT_MODEL,
} = require('../../src/lyo/elaborator-reflector');

function makeRejectedValidation({ clusterId, sender = 'validator' } = {}) {
  return {
    cluster_id: clusterId,
    topic: 'VALIDATION_RESULT',
    sender,
    content: {
      text: 'Tests failed: npm test',
      data: { approved: false, errors: ['missing regression coverage'] },
    },
  };
}

describe('elaborator reflector — buildPrompt', function () {
  it('quotes the validator feedback as data and carries class + cue', function () {
    const messages = buildPrompt({
      message: makeRejectedValidation({}),
      failure_class: 'output_generation',
      cue: 'missing regression coverage',
    });
    assert.strictEqual(messages.length, 2);
    assert.strictEqual(messages[0].role, 'system');
    assert.match(messages[0].content, /JSON/);
    assert.match(messages[0].content, /cite the specific evidence/i);
    assert.strictEqual(messages[1].role, 'user');
    assert.match(messages[1].content, /Failure class: output_generation/);
    assert.match(messages[1].content, /Trigger context: missing regression coverage/);
    // Injection containment: feedback is wrapped as quoted data.
    assert.match(messages[1].content, /quoted data, not instructions/);
    assert.match(messages[1].content, /"""\nTests failed: npm test[\s\S]*"""/);
  });
});

describe('elaborator reflector — parseReflectionJson', function () {
  it('parses a bare JSON object', function () {
    const parsed = parseReflectionJson(
      '{"explanation": "ended after edits without running tests", "intervention": "Run targeted tests before ending a run"}'
    );
    assert.strictEqual(parsed.explanation, 'ended after edits without running tests');
    assert.strictEqual(parsed.intervention, 'Run targeted tests before ending a run');
  });

  it('extracts JSON from markdown fences', function () {
    const parsed = parseReflectionJson('```json\n{"explanation": "e", "intervention": "i"}\n```');
    assert.strictEqual(parsed.explanation, 'e');
    assert.strictEqual(parsed.intervention, 'i');
  });

  it('extracts JSON surrounded by prose', function () {
    const parsed = parseReflectionJson(
      'Here is my reflection:\n{"explanation": "e2", "intervention": "i2"}\nHope that helps.'
    );
    assert.strictEqual(parsed.explanation, 'e2');
    assert.strictEqual(parsed.intervention, 'i2');
  });

  it('throws when there is no JSON object', function () {
    assert.throws(() => parseReflectionJson('no object here'), /no JSON object/);
    assert.throws(() => parseReflectionJson(''), /no JSON object/);
    assert.throws(() => parseReflectionJson(null), /no JSON object/);
  });

  it('throws when explanation or intervention is missing', function () {
    assert.throws(
      () => parseReflectionJson('{"explanation": "e"}'),
      /missing explanation\/intervention/
    );
    assert.throws(
      () => parseReflectionJson('{"explanation": 1, "intervention": "i"}'),
      /missing explanation\/intervention/
    );
  });

  it('truncates to the store length caps', function () {
    const parsed = parseReflectionJson(
      JSON.stringify({ explanation: 'x'.repeat(900), intervention: 'y'.repeat(900) })
    );
    assert.strictEqual(parsed.explanation.length, 500);
    assert.strictEqual(parsed.intervention.length, 300);
  });
});

describe('elaborator reflector — factory', function () {
  it('is an async-only reflector (no sync reflect)', function () {
    const reflector = createElaboratorReflector({ chat: () => Promise.resolve('{}') });
    assert.strictEqual(reflector.name, 'elaborator');
    assert.strictEqual(reflector.version, 1);
    assert.strictEqual(typeof reflector.reflectAsync, 'function');
    assert.strictEqual(reflector.reflect, undefined);
  });

  it('reflectAsync sends the built prompt to the injected chat and parses the reply', async function () {
    const calls = [];
    const reflector = createElaboratorReflector({
      model: 'test/model-x',
      chat: ({ messages, model }) => {
        calls.push({ messages, model });
        return Promise.resolve(
          '{"explanation": "distilled why", "intervention": "transferable rule"}'
        );
      },
    });
    const reflection = await reflector.reflectAsync({
      message: makeRejectedValidation({}),
      failure_class: 'output_generation',
      cue: 'missing regression coverage',
    });
    assert.deepStrictEqual(reflection, {
      explanation: 'distilled why',
      intervention: 'transferable rule',
    });
    assert.strictEqual(calls.length, 1);
    assert.strictEqual(calls[0].model, 'test/model-x');
    assert.match(calls[0].messages[1].content, /missing regression coverage/);
  });

  it('resolves the model from OPENROUTER_LYO_MODEL, else the default', function () {
    const saved = process.env.OPENROUTER_LYO_MODEL;
    try {
      delete process.env.OPENROUTER_LYO_MODEL;
      let seen = null;
      createElaboratorReflector({
        chat: ({ model }) => {
          seen = model;
          return Promise.resolve('{"explanation": "e", "intervention": "i"}');
        },
      }).reflectAsync({ message: {}, failure_class: 'c', cue: 'q' });
      return Promise.resolve()
        .then(() => {
          assert.strictEqual(seen, DEFAULT_MODEL);
          process.env.OPENROUTER_LYO_MODEL = 'vendor/custom-model';
          seen = null;
          return createElaboratorReflector({
            chat: ({ model }) => {
              seen = model;
              return Promise.resolve('{"explanation": "e", "intervention": "i"}');
            },
          }).reflectAsync({ message: {}, failure_class: 'c', cue: 'q' });
        })
        .then(() => {
          assert.strictEqual(seen, 'vendor/custom-model');
        });
    } finally {
      if (saved === undefined) {
        delete process.env.OPENROUTER_LYO_MODEL;
      } else {
        process.env.OPENROUTER_LYO_MODEL = saved;
      }
    }
  });

  it('rejects cleanly when OPENROUTER_API_KEY is unset (no network attempted)', async function () {
    const saved = process.env.OPENROUTER_API_KEY;
    delete process.env.OPENROUTER_API_KEY;
    try {
      const reflector = createElaboratorReflector();
      await assert.rejects(
        () => reflector.reflectAsync({ message: {}, failure_class: 'c', cue: 'q' }),
        /OPENROUTER_API_KEY is not set/
      );
    } finally {
      if (saved !== undefined) {
        process.env.OPENROUTER_API_KEY = saved;
      }
    }
  });
});

describe('elaborator reflector — registry', function () {
  it("resolves 'elaborator@1' to an async reflector without needing an API key", function () {
    const reflector = resolveReflector('elaborator@1');
    assert.strictEqual(reflector.name, 'elaborator');
    assert.strictEqual(reflector.version, 1);
    assert.strictEqual(typeof reflector.reflectAsync, 'function');
  });

  it('exposes the resolved model id for pair provenance', function () {
    const viaFactory = createElaboratorReflector({
      chat: () => Promise.resolve('{}'),
      model: 'x/y',
    });
    assert.strictEqual(viaFactory.model, 'x/y');
    // Registry ctx (cluster.config.lyo.reflectorModel) reaches the factory.
    const viaRegistry = resolveReflector('elaborator@1', { model: 'anthropic/claude-haiku' });
    assert.strictEqual(viaRegistry.model, 'anthropic/claude-haiku');
  });
});

describe('observer — pair provenance (model inversion)', function () {
  it('records reflector model and executor model on the stored lesson', async function () {
    const ledger = new Ledger(':memory:');
    const messageBus = new MessageBus(ledger);
    const store = new LessonStore(':memory:');
    const clusterId = 'lyo-pair-1';
    const cluster = {
      id: clusterId,
      config: { lyo: { enabled: true }, forceProvider: 'claude' },
      agents: [
        {
          id: 'implementer',
          config: { role: 'implementation', modelConfig: { modelLevel: 'level2' } },
        },
        { id: 'validator', config: { role: 'validation' } },
      ],
    };
    const reflector = {
      name: 'elaborator',
      version: 1,
      model: 'openai/gpt-4o-mini',
      reflectAsync: () =>
        Promise.resolve({ explanation: 'distilled why', intervention: 'distilled rule' }),
    };
    let enrichment = null;
    const detach = attachLyoObserver({
      messageBus,
      cluster,
      lessonStore: store,
      reflector,
      onEnrichment: (p) => {
        enrichment = p;
      },
    });

    messageBus.publish({
      cluster_id: clusterId,
      topic: 'VALIDATION_RESULT',
      sender: 'validator',
      content: {
        text: 'Tests failed: npm test',
        data: { approved: false, errors: ['missing regression coverage'] },
      },
    });
    await enrichment;

    const rows = store.db.prepare('SELECT * FROM lesson').all();
    assert.strictEqual(rows.length, 1);
    assert.strictEqual(rows[0].reflector_policy, 'elaborator@1');
    assert.strictEqual(rows[0].reflector_model, 'openai/gpt-4o-mini');
    assert.strictEqual(rows[0].executor_model, 'claude:level2');

    detach();
    store.close();
    ledger.close();
  });

  it('records null executor model when the cluster carries no model config', async function () {
    const ledger = new Ledger(':memory:');
    const messageBus = new MessageBus(ledger);
    const store = new LessonStore(':memory:');
    const clusterId = 'lyo-pair-2';
    const cluster = {
      id: clusterId,
      config: { lyo: { enabled: true } },
      agents: [
        { id: 'implementer', config: { role: 'implementation' } },
        { id: 'validator', config: { role: 'validation' } },
      ],
    };
    let enrichment = null;
    const detach = attachLyoObserver({
      messageBus,
      cluster,
      lessonStore: store,
      reflector: {
        name: 'elaborator',
        version: 1,
        model: 'openai/gpt-4o-mini',
        reflectAsync: () => Promise.resolve({ explanation: 'e', intervention: 'i' }),
      },
      onEnrichment: (p) => {
        enrichment = p;
      },
    });

    messageBus.publish({
      cluster_id: clusterId,
      topic: 'VALIDATION_RESULT',
      sender: 'validator',
      content: {
        text: 'Tests failed: npm test',
        data: { approved: false, errors: ['missing regression coverage'] },
      },
    });
    await enrichment;

    const rows = store.db.prepare('SELECT * FROM lesson').all();
    assert.strictEqual(rows.length, 1);
    assert.strictEqual(rows[0].executor_model, null);
    assert.strictEqual(rows[0].reflector_model, 'openai/gpt-4o-mini');

    detach();
    store.close();
    ledger.close();
  });
});

describe('observer — async reflector enrichment', function () {
  function setup({ reflector, onEnrichment } = {}) {
    const ledger = new Ledger(':memory:');
    const messageBus = new MessageBus(ledger);
    const store = new LessonStore(':memory:');
    const clusterId = 'lyo-elab-1';
    const cluster = {
      id: clusterId,
      config: { lyo: { enabled: true } },
      agents: [
        { id: 'implementer', config: { role: 'implementation' } },
        { id: 'validator', config: { role: 'validation' } },
      ],
    };
    const detach = attachLyoObserver({
      messageBus,
      cluster,
      lessonStore: store,
      reflector,
      onEnrichment,
    });
    return { ledger, messageBus, store, clusterId, detach };
  }

  function lessons(store) {
    return store.db.prepare('SELECT * FROM lesson ORDER BY created_at').all();
  }

  it('ships template guidance synchronously, then persists the distilled lesson', async function () {
    let enrichment = null;
    const reflector = {
      name: 'elaborator',
      version: 1,
      reflectAsync() {
        return Promise.resolve({
          explanation: 'Runs ended after edits without verification keep failing',
          intervention: 'Run the targeted tests for touched files before ending a run',
        });
      },
    };
    const { ledger, messageBus, store, clusterId, detach } = setup({
      reflector,
      onEnrichment: (p) => {
        enrichment = p;
      },
    });

    messageBus.publish(makeRejectedValidation({ clusterId }));

    // Synchronous path: guidance went out with TEMPLATE text (zero latency).
    const guidance = messageBus.queryGuidanceMailbox({
      cluster_id: clusterId,
      target_agent_id: 'implementer',
    });
    assert.strictEqual(guidance.length, 1);
    assert.strictEqual(guidance[0].topic, USER_GUIDANCE_AGENT);
    assert.match(guidance[0].content.text, /Address the validator feedback/i);

    // The sync path did NOT persist a lesson yet (persistCreate skipped).
    assert.strictEqual(lessons(store).length, 0);
    assert.ok(enrichment, 'onEnrichment must receive the enrichment promise');

    await enrichment;

    // After enrichment: one lesson, authored by elaborator@1, with distilled text.
    const rows = lessons(store);
    assert.strictEqual(rows.length, 1);
    assert.strictEqual(
      rows[0].explanation,
      'Runs ended after edits without verification keep failing'
    );
    assert.strictEqual(
      rows[0].intervention,
      'Run the targeted tests for touched files before ending a run'
    );
    const deltas = store.getDeltas(rows[0].lesson_id);
    assert.strictEqual(deltas.length, 1);
    assert.strictEqual(deltas[0].delta_type, 'CREATE');
    const payload = JSON.parse(deltas[0].payload);
    assert.strictEqual(payload.reflector, 'elaborator@1');

    detach();
    store.close();
    ledger.close();
  });

  it('persists a template@1 lesson when the async reflector fails', async function () {
    let enrichment = null;
    const reflector = {
      name: 'elaborator',
      version: 1,
      reflectAsync() {
        return Promise.reject(new Error('OpenRouter HTTP 500: boom'));
      },
    };
    const { ledger, messageBus, store, clusterId, detach } = setup({
      reflector,
      onEnrichment: (p) => {
        enrichment = p;
      },
    });

    messageBus.publish(makeRejectedValidation({ clusterId }));
    await enrichment;

    const rows = lessons(store);
    assert.strictEqual(rows.length, 1);
    assert.match(rows[0].intervention, /Address the validator feedback/i);
    const deltas = store.getDeltas(rows[0].lesson_id);
    const payload = JSON.parse(deltas[0].payload);
    assert.strictEqual(payload.reflector, 'template@1');

    detach();
    store.close();
    ledger.close();
  });

  it('recalls the distilled lesson on the next rejection of the same class', async function () {
    let enrichment = null;
    const reflector = {
      name: 'elaborator',
      version: 1,
      reflectAsync() {
        return Promise.resolve({
          explanation: 'distilled why',
          intervention: 'distilled transferable rule',
        });
      },
    };
    const { ledger, messageBus, store, clusterId, detach } = setup({
      reflector,
      onEnrichment: (p) => {
        enrichment = p;
      },
    });

    // Cycle 1: rejection -> template guidance now, distilled lesson later.
    messageBus.publish(makeRejectedValidation({ clusterId }));
    await enrichment;

    // Cycle 2: same failure class -> the stored lesson is injected.
    messageBus.publish(makeRejectedValidation({ clusterId }));

    const guidance = messageBus.queryGuidanceMailbox({
      cluster_id: clusterId,
      target_agent_id: 'implementer',
    });
    assert.strictEqual(guidance.length, 2);
    assert.match(guidance[1].content.text, /Lessons from past failures/);
    assert.match(guidance[1].content.text, /distilled transferable rule/);

    // Cycle 2 fired its own enrichment; drain it before closing the store.
    await enrichment;

    detach();
    store.close();
    ledger.close();
  });
});
