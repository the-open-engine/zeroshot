import test from 'node:test';
import assert from 'node:assert/strict';
import { clone, findNode, type Document, type GraphNode } from './domain';
import {
  allDataInputsMapped,
  connectData,
  dataProducerChoices,
  existingDataConnections,
  previewDataConnection,
  type DataConnectionRequest,
} from './data-connections';

const string = { kind: 'string' };
const record = (fields: Record<string, any> = {}) => ({ kind: 'record', fields });
const field = (type: any = string) => ({ type: clone(type), required: true });
const producer = (): GraphNode => ({
  kind: 'verifier',
  name: 'acceptance',
  worker: 'agent.acceptance@1',
  input: record(),
  inputBindings: [],
  output: record({ result: field() }),
  diagnostic: record({ message: field() }),
  signals: { verdict: ['accepted', 'rejected'] },
  writeBindings: [],
  instructions: 'Review',
  attempts: 1,
});
const consumer = (): GraphNode => ({
  kind: 'step',
  name: 'repair',
  worker: 'agent.repair@1',
  input: record({ feedback: field() }),
  inputBindings: [],
  output: { kind: 'null' },
  writeBindings: [],
  instructions: 'Repair',
  attempts: 1,
});
const seq = (name: string, children: GraphNode[]): GraphNode => ({
  kind: 'seq',
  name,
  state: record(),
  children,
  promotedStatePaths: [],
});
function document(root = seq('run', [producer(), consumer()])): Document {
  return {
    name: 'test',
    graph: { profile: 'full', initialInput: record(), policy: {}, root },
    runtime: {
      harness: 'codex',
      provider: 'openai',
      size: 'small',
      nodes: {
        acceptance: { kind: 'agent', model: 'opaque', connections: { tools: ['TOKEN'] } },
        repair: { kind: 'agent', model: 'opaque' },
      },
    },
  };
}
const request: DataConnectionRequest = {
  consumer: 'repair',
  target: ['feedback'],
  source: { node: 'acceptance', channel: 'diagnostic', path: ['message'] },
};

test('direct connection appends a dedicated optional state field and applies one immutable edit', () => {
  const doc = document();
  const before = clone(doc);
  const plan = previewDataConnection(doc, request);
  assert.equal(plan.canConnect, true, plan.reasons.join('; '));
  assert.deepEqual(plan.scopes, ['run']);
  assert.deepEqual(plan.promotions, []);
  const next = connectData(doc, request);
  assert.deepEqual(doc, before);
  assert.deepEqual(next.runtime, before.runtime);
  assert.deepEqual(next.graph.initialInput, before.graph.initialInput);
  assert.deepEqual(next.graph.root.state.fields[plan.statePath[0]], {
    type: string,
    required: false,
  });
  assert.deepEqual(findNode(next.graph.root, 'acceptance')!.writeBindings, [
    { target: plan.statePath, value: request.source },
  ]);
  assert.deepEqual(findNode(next.graph.root, 'repair')!.inputBindings, [
    { target: ['feedback'], value: { source: 'state', path: plan.statePath } },
  ]);
  assert.deepEqual(findNode(next.graph.root, 'repair')!.input, consumer().input);
  assert.throws(() => connectData(next, request), /already has a mapping/);
});

test('nested sequences add state along both sides but promote only out of producer scopes', () => {
  const source = producer();
  source.writeBindings.push({
    target: ['old'],
    value: { node: 'acceptance', channel: 'out', path: ['result'] },
    extension: 'preserve',
  });
  const left = seq('left', [seq('deep', [source])]);
  left.state = { ...record({ old: field() }), extension: { keep: true } };
  left.promotedStatePaths = [['old']];
  const right = seq('right', [consumer()]);
  const doc = document(seq('run', [left, right]));
  const plan = previewDataConnection(doc, request);
  assert.equal(plan.canConnect, true, plan.reasons.join('; '));
  assert.deepEqual(plan.scopes, ['run', 'left', 'deep', 'right']);
  assert.deepEqual(plan.promotions, ['deep', 'left']);
  const next = connectData(doc, request);
  assert.deepEqual(findNode(next.graph.root, 'right')!.promotedStatePaths, []);
  assert.deepEqual(findNode(next.graph.root, 'left')!.promotedStatePaths, [
    ['old'],
    plan.statePath,
  ]);
  assert.deepEqual(findNode(next.graph.root, 'left')!.state.extension, { keep: true });
  assert.deepEqual(
    findNode(next.graph.root, 'acceptance')!.writeBindings[0],
    source.writeBindings[0]
  );
});

function reviewDocument(): Document {
  const parallel = {
    kind: 'par',
    name: 'reviews',
    state: record(),
    branches: [producer()],
    promotedStatePaths: [],
    join: { kind: 'all' },
  };
  const choice = {
    kind: 'choice',
    name: 'review_result',
    state: record(),
    branches: [
      {
        when: {
          kind: 'in',
          value: { name: 'acceptance', source: 'error', field: null },
          labels: ['crash', 'timeout', 'malformed', 'refusal'],
        },
        node: { kind: 'fail', name: 'failed', reason: 'failed' },
      },
    ],
    otherwise: consumer(),
    promotedStatePaths: [],
  };
  const iteration = seq('iteration', [parallel, choice]);
  const loop = {
    kind: 'loop',
    name: 'retry',
    state: record(),
    body: iteration,
    maxIterations: 3,
    promotedStatePaths: [],
  };
  return document(seq('run', [loop]));
}

test('parallel-all diagnostic reuses an existing complete route into a later choice', () => {
  const doc = reviewDocument();
  const fresh = previewDataConnection(doc, request);
  assert.equal(fresh.canConnect, false);
  assert.ok(fresh.reasons.some((reason) => reason.includes('parallel producer')));
  for (const name of ['iteration', 'reviews', 'review_result'])
    findNode(doc.graph.root, name)!.state = record({ acceptanceFeedback: field() });
  findNode(doc.graph.root, 'reviews')!.promotedStatePaths = [['acceptanceFeedback']];
  findNode(doc.graph.root, 'acceptance')!.writeBindings = [
    { target: ['acceptanceFeedback'], value: request.source },
  ];
  const before = clone(doc);
  const plan = previewDataConnection(doc, request);
  assert.equal(plan.canConnect, true, plan.reasons.join('; '));
  assert.equal(plan.reusesState, true);
  assert.deepEqual(plan.scopes, []);
  assert.deepEqual(plan.promotions, []);
  assert.deepEqual(plan.statePath, ['acceptanceFeedback']);
  assert.ok(plan.notes.some((note) => note.includes('conditionally')));
  const next = connectData(doc, request);
  assert.deepEqual(next.graph.root.state, doc.graph.root.state);
  assert.deepEqual(
    findNode(next.graph.root, 'retry')!.state,
    findNode(doc.graph.root, 'retry')!.state
  );
  const traced = existingDataConnections(next, findNode(next.graph.root, 'repair')!);
  assert.deepEqual(traced[0].producers, [
    { name: 'acceptance', label: 'acceptance · diagnostic · message' },
  ]);
  const restored = clone(next);
  findNode(restored.graph.root, 'repair')!.inputBindings = [];
  assert.deepEqual(restored, before, 'reuse only appends the consumer binding');
});

test('multiple existing paths or competing writers are not silently selected for reuse', () => {
  const doc = reviewDocument();
  for (const name of ['iteration', 'reviews', 'review_result'])
    findNode(doc.graph.root, name)!.state = record({ first: field(), second: field() });
  findNode(doc.graph.root, 'reviews')!.promotedStatePaths = [['first'], ['second']];
  findNode(doc.graph.root, 'acceptance')!.writeBindings = [
    { target: ['first'], value: request.source },
    { target: ['second'], value: request.source },
  ];
  assert.equal(previewDataConnection(doc, request).canConnect, false);
  assert.ok(
    previewDataConnection(doc, request).reasons.some((reason) =>
      reason.includes('Multiple authored')
    )
  );
  findNode(doc.graph.root, 'acceptance')!.writeBindings.pop();
  findNode(doc.graph.root, 'repair')!.writeBindings = [
    { target: ['first'], value: { node: 'repair', channel: 'out', path: ['other'] } },
  ];
  assert.equal(previewDataConnection(doc, request).canConnect, false);
});

test('existing template-style mappings trace through group returns without changing authored paths', () => {
  const doc = reviewDocument();
  for (const name of ['iteration', 'reviews', 'review_result'])
    findNode(doc.graph.root, name)!.state = record({ acceptanceFeedback: field() });
  findNode(doc.graph.root, 'reviews')!.promotedStatePaths = [['acceptanceFeedback']];
  findNode(doc.graph.root, 'acceptance')!.writeBindings = [
    { target: ['acceptanceFeedback'], value: request.source },
  ];
  findNode(doc.graph.root, 'repair')!.inputBindings = [
    { target: ['feedback'], value: { source: 'state', path: ['acceptanceFeedback'] } },
  ];
  const before = clone(doc);
  const traced = existingDataConnections(doc, findNode(doc.graph.root, 'repair')!);
  assert.equal(traced[0].producers[0].name, 'acceptance');
  assert.equal(previewDataConnection(doc, request).canConnect, false);
  assert.deepEqual(doc, before);
  findNode(doc.graph.root, 'reviews')!.promotedStatePaths = [];
  assert.deepEqual(
    existingDataConnections(doc, findNode(doc.graph.root, 'repair')!)[0].producers,
    []
  );
});

test('unavailable ordering and scope routes report reasons and remain unchanged', () => {
  const cases: GraphNode[] = [
    seq('run', [consumer(), producer()]),
    {
      kind: 'par',
      name: 'run',
      state: record(),
      branches: [producer(), consumer()],
      promotedStatePaths: [],
      join: { kind: 'all' },
    },
    seq('run', [
      {
        kind: 'choice',
        name: 'conditional',
        state: record(),
        branches: [{ when: { kind: 'future' }, node: producer() }],
        promotedStatePaths: [],
      },
      consumer(),
    ]),
    seq('run', [
      {
        kind: 'par',
        name: 'partial',
        state: record(),
        branches: [producer()],
        promotedStatePaths: [],
        join: { kind: 'any' },
      },
      consumer(),
    ]),
    seq('run', [
      { kind: 'loop', name: 'repeated', state: record(), body: producer(), promotedStatePaths: [] },
      consumer(),
    ]),
    seq('run', [
      producer(),
      { kind: 'map', name: 'items', state: record(), body: consumer(), promotedStatePaths: [] },
    ]),
  ];
  for (const root of cases) {
    const doc = document(root);
    const before = clone(doc);
    const plan = previewDataConnection(doc, request);
    assert.equal(plan.canConnect, false);
    assert.ok(plan.reasons.length);
    assert.throws(() => connectData(doc, request));
    assert.deepEqual(doc, before);
  }
});

test('fresh paths avoid authored fields and dangling references, including literal dotted field names', () => {
  const doc = document();
  const first = previewDataConnection(doc, request).statePath[0];
  doc.graph.root.state.fields[first] = field();
  findNode(doc.graph.root, 'acceptance')!.writeBindings = [
    { target: [`${first}_2`], value: { future: true } },
  ];
  const plan = previewDataConnection(doc, request);
  assert.equal(plan.statePath[0], `${first}_3`);
  findNode(doc.graph.root, 'acceptance')!.diagnostic = record({ 'message.text': field() });
  findNode(doc.graph.root, 'repair')!.input = record({ 'feedback.text': field() });
  const literal = {
    consumer: 'repair',
    target: ['feedback.text'],
    source: { node: 'acceptance', channel: 'diagnostic' as const, path: ['message.text'] },
  };
  const next = connectData(doc, literal);
  assert.deepEqual(findNode(next.graph.root, 'repair')!.inputBindings[0].target, ['feedback.text']);
  assert.deepEqual(findNode(next.graph.root, 'acceptance')!.writeBindings[1].value.path, [
    'message.text',
  ]);
});

test('unsupported mappings and optional or incompatible producer types cannot be overwritten', () => {
  for (const mutate of [
    (doc: Document) => {
      doc.graph.root.state = { kind: 'null' };
    },
    (doc: Document) => {
      findNode(doc.graph.root, 'repair')!.inputBindings = { future: [] };
    },
    (doc: Document) => {
      findNode(doc.graph.root, 'repair')!.inputBindings = [{ target: { bad: true }, value: null }];
    },
    (doc: Document) => {
      findNode(doc.graph.root, 'acceptance')!.writeBindings = { future: [] };
    },
    (doc: Document) => {
      findNode(doc.graph.root, 'acceptance')!.diagnostic.fields.message.required = false;
    },
    (doc: Document) => {
      findNode(doc.graph.root, 'repair')!.input.fields.feedback.type = { kind: 'boolean' };
    },
  ]) {
    const doc = document();
    mutate(doc);
    const before = clone(doc);
    assert.equal(previewDataConnection(doc, request).canConnect, false);
    assert.throws(() => connectData(doc, request));
    assert.deepEqual(doc, before);
  }
});

test('success output and verifier signals use the native bindings and enum wire shape', () => {
  const done = {
    kind: 'succeed',
    name: 'done',
    output: record({ verdict: field({ kind: 'enum', values: ['accepted', 'rejected'] }) }),
    bindings: [],
  };
  const doc = document(seq('run', [producer(), done]));
  const signal: DataConnectionRequest = {
    consumer: 'done',
    target: ['verdict'],
    source: { node: 'acceptance', channel: 'signal', path: ['verdict'] },
  };
  const next = connectData(doc, signal);
  assert.deepEqual(findNode(next.graph.root, 'done')!.bindings[0].target, ['verdict']);
  assert.equal(findNode(next.graph.root, 'done')!.inputBindings, undefined);
  assert.ok(
    dataProducerChoices(doc, done).some(
      (choice) => choice.source.channel === 'signal' && choice.available
    )
  );
});

function runInputDocument(): Document {
  const doc = document(seq('run', [consumer(), producer()]));
  doc.graph.initialInput = record({ task: field() });
  doc.graph.root.state = record({ task: field() });
  findNode(doc.graph.root, 'repair')!.input = record({ task: field() });
  findNode(doc.graph.root, 'repair')!.inputBindings = [
    { target: ['task'], value: { source: 'state', path: ['task'] } },
  ];
  return doc;
}

test('required untouched fields are presented as run input without changing their state mappings', () => {
  const doc = runInputDocument();
  // This later producer cannot overwrite the first consumer's input.
  findNode(doc.graph.root, 'acceptance')!.writeBindings = [
    { target: ['task'], value: request.source },
  ];
  const before = clone(doc);
  const connection = existingDataConnections(doc, findNode(doc.graph.root, 'repair')!)[0];
  assert.equal(connection.source, 'Run input · task');
  assert.equal(connection.note, 'Supplied when the run starts and passed unchanged to this node.');
  assert.deepEqual(connection.producers, []);
  assert.deepEqual(doc, before);
});

test('run input presentation requires a required path with matching schemas and no prior scoped overwrite', () => {
  const optional = runInputDocument();
  optional.graph.initialInput.fields.task.required = false;
  assert.equal(
    existingDataConnections(optional, findNode(optional.graph.root, 'repair')!)[0].source,
    'State · task'
  );
  const mismatch = runInputDocument();
  mismatch.graph.root.state.fields.task.type = { kind: 'boolean' };
  assert.equal(
    existingDataConnections(mismatch, findNode(mismatch.graph.root, 'repair')!)[0].source,
    'State · task'
  );

  for (const write of [
    [{ target: ['task'], value: { future: true } }],
    [{ target: { unknown: true }, value: null }],
    { unknown: [] },
  ]) {
    const prior = runInputDocument();
    prior.graph.root.children.reverse();
    findNode(prior.graph.root, 'acceptance')!.writeBindings = write;
    assert.equal(
      existingDataConnections(prior, findNode(prior.graph.root, 'repair')!)[0].source,
      'State · task'
    );
  }
});

test('non-promoted child writes stay local, while conditional promotions obscure run-input origin', () => {
  const doc = runInputDocument();
  const writer = producer();
  writer.writeBindings = [{ target: ['task'], value: request.source }];
  const branch = {
    kind: 'choice',
    name: 'conditional',
    state: record({ task: field() }),
    branches: [{ when: { kind: 'future' }, node: writer }],
    promotedStatePaths: [] as string[][],
  };
  doc.graph.root.children = [branch, consumer()];
  findNode(doc.graph.root, 'repair')!.input = record({ task: field() });
  findNode(doc.graph.root, 'repair')!.inputBindings = [
    { target: ['task'], value: { source: 'state', path: ['task'] } },
  ];
  assert.equal(
    existingDataConnections(doc, findNode(doc.graph.root, 'repair')!)[0].source,
    'Run input · task'
  );
  branch.promotedStatePaths = [['task']];
  assert.equal(
    existingDataConnections(doc, findNode(doc.graph.root, 'repair')!)[0].source,
    'State · task'
  );
});

test('later writes in repeated scopes prevent an untouched-run-input claim', () => {
  const doc = runInputDocument();
  const body = doc.graph.root;
  body.name = 'iteration';
  findNode(body, 'acceptance')!.writeBindings = [{ target: ['task'], value: request.source }];
  doc.graph.root = {
    kind: 'loop',
    name: 'run',
    state: record({ task: field() }),
    body,
    promotedStatePaths: [],
    maxIterations: 3,
  };
  assert.equal(
    existingDataConnections(doc, findNode(doc.graph.root, 'repair')!)[0].source,
    'State · task'
  );
});

test('fully mapped inputs include record coverage and collapse only when every leaf is covered', () => {
  const node = consumer();
  node.input = record({ payload: field(record({ first: field(), second: field() })) });
  node.inputBindings = [
    { target: ['payload', 'first'], value: { source: 'state', path: ['one'] } },
  ];
  assert.equal(allDataInputsMapped(node), false);
  node.inputBindings.push({
    target: ['payload', 'second'],
    value: { source: 'state', path: ['two'] },
  });
  assert.equal(allDataInputsMapped(node), true);
  node.inputBindings = [{ target: ['payload'], value: { source: 'state', path: ['object'] } }];
  assert.equal(allDataInputsMapped(node), true);
  node.inputBindings = { future: true };
  assert.equal(allDataInputsMapped(node), false);
  node.input = record();
  assert.equal(allDataInputsMapped(node), false);
});
