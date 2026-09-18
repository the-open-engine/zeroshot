import test from 'node:test';
import assert from 'node:assert/strict';
import { type Document, type GraphNode, findNode } from './domain';
import {
  choiceOutputFields,
  editOutputField,
  inputSourceId,
  inputSourceLabel,
  mapSourceLabel,
  nodeDataChoices,
  nodeOutputFields,
  removeOutputField,
  renameInputField,
} from './node-data';

const string = { kind: 'string' };
const record = (fields: Record<string, any>) => ({
  kind: 'record',
  fields: Object.fromEntries(
    Object.entries(fields).map(([name, type]) => [name, { type, required: true }])
  ),
});
const state = record({ task: string, plan: string });
const work = (name: string): GraphNode => ({
  kind: 'step',
  name,
  input: record({ task: string }),
  output: record({ plan: string }),
  inputBindings: [{ target: ['task'], value: { source: 'state', path: ['task'] } }],
  writeBindings: [{ target: ['plan'], value: { node: name, channel: 'out', path: ['plan'] } }],
});
const seq = (name: string, children: GraphNode[]): GraphNode => ({
  kind: 'seq',
  name,
  state: structuredClone(state),
  children,
  promotedStatePaths: [['plan']],
});
const doc = (root: GraphNode): Document => ({
  name: 'data',
  graph: { profile: 'test', root, initialInput: record({ task: string }), policy: {} },
  runtime: { harness: '', provider: '', size: 'small', nodes: {} },
});
const reader = (name = 'reader'): GraphNode => ({
  ...work(name),
  input: record({ task: string, brief: string }),
  inputBindings: [
    { target: ['task'], value: { source: 'state', path: ['task'] } },
    { target: ['brief'], value: { source: 'state', path: ['plan'] } },
  ],
  writeBindings: [],
});

test('one output model includes data, review details and outcomes while retaining native channels', () => {
  const node = {
    ...work('review'),
    kind: 'verifier',
    diagnostic: record({ feedback: string }),
    signals: { verdict: ['accepted', 'rejected'] },
  };
  const before = structuredClone(node);
  assert.deepEqual(
    nodeOutputFields(node).map((field) => [field.name, field.channel, field.type.kind]),
    [
      ['plan', 'out', 'string'],
      ['feedback', 'diagnostic', 'string'],
      ['verdict', 'signal', 'enum'],
    ]
  );
  assert.deepEqual(node, before);
});

test('source choices offer run inputs, earlier producers and map items but omit future/parallel peers', () => {
  const first = work('first'),
    second = work('second'),
    later = work('later');
  const document = doc(seq('run', [first, second, later]));
  assert.deepEqual(
    nodeDataChoices(document, second).map((choice) => choice.label),
    ['Run input · task', 'first · plan']
  );
  const parallel = doc({
    kind: 'par',
    name: 'together',
    state,
    branches: [first, second],
    join: { kind: 'all' },
  });
  assert.equal(
    nodeDataChoices(parallel, second).some((choice) => choice.source.kind === 'node_output'),
    false
  );
  const mapped = doc(
    seq('run', [
      {
        kind: 'map',
        name: 'items',
        state: record({ items: { kind: 'array', items: record({ title: string }) } }),
        over: { source: 'state', path: ['items'] },
        body: first,
        maxItems: 5,
      },
    ])
  );
  assert.ok(nodeDataChoices(mapped, first).some((choice) => choice.label === 'Item · title'));
});

test('saved routes display their producer or proven run input and preserve unknown mappings', () => {
  const writer = work('writer'),
    consumer = reader();
  const document = doc(seq('run', [writer, consumer]));
  const before = structuredClone(document);
  assert.equal(inputSourceLabel(document, consumer, 'task'), 'Run input · task');
  assert.equal(inputSourceLabel(document, consumer, 'brief'), 'writer · plan');
  const choices = nodeDataChoices(document, consumer);
  assert.equal(
    inputSourceId(document, consumer, 'brief', choices),
    choices.find((choice) => choice.label === 'writer · plan')!.id
  );
  consumer.inputBindings.push({ target: ['mystery'], value: { source: 'future', path: ['x'] } });
  assert.equal(inputSourceLabel(document, consumer, 'mystery'), 'Saved source');
  assert.match(inputSourceId(document, consumer, 'mystery', choices), /^saved:/);
  consumer.inputBindings.pop();
  assert.deepEqual(document, before);
});

test('overwritten caller slots are not advertised as original run input choices', () => {
  const writer = work('writer'),
    consumer = reader();
  writer.writeBindings[0].target = ['task'];
  const document = doc(seq('run', [writer, consumer]));
  assert.equal(
    nodeDataChoices(document, consumer).some((choice) => choice.source.kind === 'run_input'),
    false
  );
  assert.notEqual(inputSourceLabel(document, consumer, 'task'), 'Run input · task');
});

test('choice offers its common result instead of individual alternative producers', () => {
  const left = work('left'),
    right = work('right');
  const choice: GraphNode = {
    kind: 'choice',
    name: 'route',
    state,
    promotedStatePaths: [['plan']],
    branches: [{ when: {}, node: left }],
    otherwise: right,
  };
  assert.deepEqual(
    choiceOutputFields(choice).map((field) => field.name),
    ['plan']
  );
  const consumer = reader('after'),
    document = doc(seq('run', [choice, consumer]));
  const choices = nodeDataChoices(document, consumer);
  assert.ok(choices.some((option) => option.label === 'route · plan'));
  assert.equal(
    choices.some((option) => option.label === 'left · plan'),
    false
  );
  assert.equal(inputSourceLabel(document, consumer, 'brief'), 'route · plan');
  right.output = record({ plan: { kind: 'number' } });
  assert.deepEqual(choiceOutputFields(choice), []);
});

test('loop feedback labels identify the previous attempt and require authored return paths', () => {
  const writer = work('review'),
    consumer = reader('revise');
  const body = seq('round', [consumer, writer]);
  const document = doc(
    seq('run', [
      {
        kind: 'loop',
        name: 'rounds',
        state,
        body,
        maxIterations: 3,
        promotedStatePaths: [['plan']],
      },
    ])
  );
  assert.equal(inputSourceLabel(document, consumer, 'brief'), 'review · plan (previous attempt)');
  body.promotedStatePaths = [];
  assert.equal(inputSourceLabel(document, consumer, 'brief'), 'Current value · plan');
});

function loopFeedbackFixture() {
  const donor = reader('draft'),
    receiver = work('assemble'),
    repair = work('repair');
  receiver.input = { kind: 'null' };
  receiver.inputBindings = [];
  receiver.writeBindings = [];
  donor.writeBindings = [];
  const body = seq('round', [donor, receiver, repair]);
  const loop: GraphNode = {
    kind: 'loop',
    name: 'rounds',
    state: structuredClone(state),
    body,
    maxIterations: 3,
    promotedStatePaths: [['plan']],
  };
  const document = doc(seq('run', [loop]));
  return { document, donor, receiver, repair, body, loop };
}

test('a second consumer can select an already-authored previous-attempt input without offering arbitrary future outputs', () => {
  const { document, receiver, donor } = loopFeedbackFixture();
  const before = structuredClone(document);
  const choices = nodeDataChoices(document, receiver);
  const feedback = choices.filter((choice) => choice.source.kind === 'loop_input');
  assert.deepEqual(
    feedback.map((choice) => choice.label),
    ['repair · plan (previous attempt)']
  );
  assert.deepEqual(feedback[0].source, { kind: 'loop_input', node: 'draft', path: ['brief'] });
  assert.equal(
    choices.some(
      (choice) => choice.source.kind === 'node_output' && choice.source.node === 'repair'
    ),
    false
  );
  assert.deepEqual(document, before);
  assert.equal(
    inputSourceId(document, donor, 'brief', nodeDataChoices(document, donor)),
    feedback[0].id
  );
});

test('feedback choices require established promotions, required scope types and one unambiguous writer', () => {
  for (const damage of [
    ({ body }: ReturnType<typeof loopFeedbackFixture>) => {
      body.promotedStatePaths = [];
    },
    ({ body }: ReturnType<typeof loopFeedbackFixture>) => {
      body.state.fields.plan.required = false;
    },
    ({ donor }: ReturnType<typeof loopFeedbackFixture>) => {
      donor.input.fields.brief.required = false;
    },
    ({ receiver }: ReturnType<typeof loopFeedbackFixture>) => {
      receiver.writeBindings = [
        { target: ['plan', 'nested'], value: { node: 'assemble', channel: 'out', path: ['plan'] } },
      ];
    },
    ({ document }: ReturnType<typeof loopFeedbackFixture>) => {
      document.graph.root.state.fields.plan.type = { kind: 'number' };
    },
  ]) {
    const fixture = loopFeedbackFixture();
    damage(fixture);
    assert.equal(
      nodeDataChoices(fixture.document, fixture.receiver).some(
        (choice) => choice.source.kind === 'loop_input'
      ),
      false
    );
  }
});

test('feedback sources do not cross another loop, a map, or a parallel peer writer', () => {
  for (const kind of ['loop', 'map', 'par']) {
    const { document, receiver, repair, body } = loopFeedbackFixture();
    if (kind === 'par') {
      body.children = [
        body.children[0],
        {
          kind: 'par',
          name: 'peers',
          state: structuredClone(state),
          branches: [receiver, repair],
          join: { kind: 'all' },
          promotedStatePaths: [['plan']],
        },
      ];
    } else {
      body.children[1] = {
        kind,
        name: 'inner',
        state: structuredClone(state),
        body: receiver,
        maxIterations: 2,
        maxItems: 2,
        over: { source: 'state', path: ['items'] },
        promotedStatePaths: [],
      };
    }
    assert.equal(
      nodeDataChoices(document, receiver).some((choice) => choice.source.kind === 'loop_input'),
      false
    );
  }
});

test('parallel readers may share existing feedback from a later sequential repair activity', () => {
  const { document, donor, receiver, body, repair } = loopFeedbackFixture();
  const peers: GraphNode = {
    kind: 'par',
    name: 'drafts',
    state: structuredClone(state),
    branches: [donor, { ...work('other'), writeBindings: [] }],
    join: { kind: 'all' },
    promotedStatePaths: [],
  };
  body.children = [peers, receiver, repair];
  assert.ok(
    nodeDataChoices(document, receiver).some((choice) => choice.source.kind === 'loop_input')
  );
});

test('map and parallel returned values identify the actual producer', () => {
  for (const group of [
    {
      kind: 'map',
      name: 'items',
      state: record({ plan: { kind: 'array', items: string } }),
      body: work('writer'),
      promotedStatePaths: [['plan']],
      over: { source: 'state', path: ['items'] },
    },
    {
      kind: 'par',
      name: 'parallel',
      state,
      branches: [work('writer')],
      promotedStatePaths: [['plan']],
      join: { kind: 'all' },
    },
  ]) {
    const consumer = reader();
    const document = doc(seq('run', [group, consumer]));
    assert.equal(inputSourceLabel(document, consumer, 'brief'), 'writer · plan');
  }
});

test('outcome renames repair exact guard and output selectors while preserving field routing', () => {
  const reviewer: GraphNode = {
    ...work('review'),
    kind: 'verifier',
    diagnostic: { kind: 'null' },
    signals: { verdict: ['accepted', 'rejected'] },
  };
  reviewer.writeBindings.push({
    target: ['accepted'],
    value: { node: 'review', channel: 'signal', path: ['verdict'] },
  });
  const route: GraphNode = {
    kind: 'choice',
    name: 'route',
    branches: [
      {
        when: {
          kind: 'in',
          value: { name: 'review', source: 'signal', field: 'verdict' },
          labels: ['accepted'],
        },
        node: { kind: 'succeed', name: 'done' },
      },
    ],
    otherwise: { kind: 'fail', name: 'failed', reason: 'failed' },
  };
  const document = doc(seq('run', [reviewer, route])),
    before = structuredClone(document);
  const field = nodeOutputFields(reviewer).find((field) => field.channel === 'signal')!;
  const next = editOutputField(document, 'review', field, { ...field, name: 'decision' });
  assert.deepEqual(findNode(next.graph.root, 'review')!.signals, {
    decision: ['accepted', 'rejected'],
  });
  assert.equal(findNode(next.graph.root, 'route')!.branches[0].when.value.field, 'decision');
  assert.deepEqual(findNode(next.graph.root, 'review')!.writeBindings[1], {
    target: ['accepted'],
    value: { node: 'review', channel: 'signal', path: ['decision'] },
  });
  assert.deepEqual(document, before);
});

test('input aliases rename targets without changing their source or unrelated fields', () => {
  const document = doc(seq('run', [work('work')]));
  const next = renameInputField(document, 'work', 'task', 'request');
  assert.deepEqual(findNode(next.graph.root, 'work')!.input, record({ request: string }));
  assert.deepEqual(findNode(next.graph.root, 'work')!.inputBindings, [
    { target: ['request'], value: { source: 'state', path: ['task'] } },
  ]);
  assert.deepEqual(next.graph.initialInput, document.graph.initialInput);
});

test('output type changes update only connected schemas through exact routes', () => {
  const writer = work('writer'),
    consumer = reader(),
    unrelated = seq('unrelated', [{ ...reader('other_reader'), inputBindings: [] }]);
  unrelated.promotedStatePaths = [];
  const document = doc(seq('run', [seq('production', [writer]), consumer, unrelated]));
  const original = structuredClone(document);
  const field = nodeOutputFields(writer)[0];
  const next = editOutputField(document, 'writer', field, { ...field, type: { kind: 'number' } });
  assert.equal(findNode(next.graph.root, 'production')!.state.fields.plan.type.kind, 'number');
  assert.equal(next.graph.root.state.fields.plan.type.kind, 'number');
  assert.equal(findNode(next.graph.root, 'reader')!.input.fields.brief.type.kind, 'number');
  assert.deepEqual(findNode(next.graph.root, 'unrelated')!.state, unrelated.state);
  assert.deepEqual(document, original);
});

test('output deletion unbinds connected inputs, retains their type, and clears uniquely owned writes/returns', () => {
  const writer = work('writer'),
    consumer = reader();
  const document = doc(seq('run', [seq('production', [writer]), consumer]));
  const next = removeOutputField(document, 'writer', nodeOutputFields(writer)[0]);
  assert.deepEqual(findNode(next.graph.root, 'writer')!.writeBindings, []);
  assert.deepEqual(findNode(next.graph.root, 'production')!.promotedStatePaths, []);
  assert.equal(findNode(next.graph.root, 'reader')!.input.fields.brief.type.kind, 'string');
  assert.deepEqual(
    findNode(next.graph.root, 'reader')!.inputBindings,
    consumer.inputBindings.slice(0, 1)
  );
  assert.equal(
    inputSourceLabel(next, findNode(next.graph.root, 'reader')!, 'brief'),
    'Choose source'
  );
});

test('unknown sibling schemas and runtime metadata survive supported output edits', () => {
  const node: GraphNode = {
    ...work('work'),
    output: {
      kind: 'record',
      future: 42,
      fields: {
        plan: { type: string, required: true, extension: true },
        future: { type: { kind: 'future', arbitrary: true }, required: false },
      },
    },
  };
  const document = doc(seq('run', [node]));
  const field = nodeOutputFields(node).find((field) => field.name === 'plan')!;
  const next = editOutputField(document, 'work', field, { ...field, name: 'brief' });
  assert.deepEqual(
    findNode(next.graph.root, 'work')!.output.fields.future,
    node.output.fields.future
  );
  assert.equal(findNode(next.graph.root, 'work')!.output.fields.brief.extension, true);
  assert.equal(findNode(next.graph.root, 'work')!.output.future, 42);
  assert.deepEqual(next.runtime, document.runtime);
});

function collectionDocument() {
  const items = { kind: 'array', items: record({ title: string }) };
  const producer = {
    ...work('collect'),
    output: record({ items }),
    writeBindings: [
      { target: ['__ui_data_1'], value: { node: 'collect', channel: 'out', path: ['items'] } },
    ],
  };
  const consumer = {
    ...work('draft'),
    input: record({ subject: string }),
    inputBindings: [{ target: ['subject'], value: { source: 'item', path: ['title'] } }],
    writeBindings: [],
  };
  const map = {
    kind: 'map',
    name: 'each',
    state: record({ __ui_data_1: items }),
    over: { source: 'state', path: ['__ui_data_1'] },
    body: consumer,
    promotedStatePaths: [],
  };
  const document = doc(seq('run', [producer, map]));
  document.graph.root.state = record({ task: string, __ui_data_1: items });
  return document;
}

function protectedCollectionDocument() {
  const document = collectionDocument();
  const [producer, map] = document.graph.root.children;
  const carriedState = structuredClone(document.graph.root.state);
  carriedState.fields.__ui_data_1.required = false;
  carriedState.fields.__ui_data_2 = structuredClone(carriedState.fields.__ui_data_1);
  const carry = (name: string, children: GraphNode[]): GraphNode => ({
    kind: 'seq',
    name,
    state: structuredClone(carriedState),
    children,
    promotedStatePaths: [],
  });
  const protect = (name: string, watched: string, continuation: GraphNode): GraphNode => ({
    kind: 'choice',
    name,
    state: structuredClone(carriedState),
    promotedStatePaths: [],
    branches: [
      {
        when: {
          kind: 'in',
          value: { name: watched, source: 'error' },
          labels: ['timeout', 'crash', 'malformed', 'refusal'],
        },
        node: { kind: 'fail', name: `${name}_failed`, reason: 'execution_failed' },
      },
    ],
    otherwise: continuation,
  });
  map.state = structuredClone(carriedState);
  map.body = carry('map_body', [
    { ...map.body, kind: 'verifier', diagnostic: { kind: 'null' }, signals: {} },
  ]);
  document.graph.root = carry('run', [
    producer,
    protect(
      'on_error',
      producer.name,
      carry('continuation', [
        map,
        protect(
          'on_error_1',
          map.name,
          carry('continuation_1', [
            { kind: 'succeed', name: 'done', output: { kind: 'null' }, bindings: [] },
          ])
        ),
      ])
    ),
  ]);
  return document;
}

test('generated output types follow carried state through error continuations after the last consumer', () => {
  const document = protectedCollectionDocument(),
    original = structuredClone(document);
  const field = nodeOutputFields(findNode(document.graph.root, 'collect')!)[0];
  const next = editOutputField(document, 'collect', field, {
    ...field,
    type: { kind: 'array', items: record({ title: { kind: 'number' } }) },
  });
  for (const name of [
    'run',
    'on_error',
    'continuation',
    'each',
    'map_body',
    'on_error_1',
    'continuation_1',
  ]) {
    const state = findNode(next.graph.root, name)!.state;
    assert.equal(state.fields.__ui_data_1.type.items.fields.title.type.kind, 'number', name);
    assert.equal(state.fields.__ui_data_1.required, false, name);
    assert.equal(state.fields.__ui_data_2.type.items.fields.title.type.kind, 'string', name);
  }
  assert.equal(findNode(next.graph.root, 'draft')!.input.fields.subject.type.kind, 'number');
  assert.deepEqual(
    findNode(next.graph.root, 'on_error_1')!.branches,
    findNode(original.graph.root, 'on_error_1')!.branches
  );
  assert.deepEqual(document, original);
});

test('carried generated output edits do not cross earlier scopes or a local competing write', () => {
  const document = protectedCollectionDocument();
  const state = structuredClone(document.graph.root.state);
  const before = { ...seq('earlier', []), state: structuredClone(state), promotedStatePaths: [] };
  document.graph.root.children.unshift(before);
  const local = {
    ...work('local'),
    output: record({ items: state.fields.__ui_data_1.type }),
    writeBindings: [
      { target: ['__ui_data_1'], value: { node: 'local', channel: 'out', path: ['items'] } },
    ],
  };
  const isolated = {
    ...seq('local_scope', [local]),
    state: structuredClone(state),
    promotedStatePaths: [],
  };
  findNode(document.graph.root, 'continuation_1')!.children.unshift(isolated);
  const field = nodeOutputFields(findNode(document.graph.root, 'collect')!)[0];
  const next = editOutputField(document, 'collect', field, {
    ...field,
    type: { kind: 'array', items: record({ title: { kind: 'number' } }) },
  });
  assert.deepEqual(findNode(next.graph.root, 'earlier')!.state, before.state);
  assert.deepEqual(findNode(next.graph.root, 'local_scope')!.state, isolated.state);
  assert.deepEqual(findNode(next.graph.root, 'local')!, local);
  assert.equal(
    findNode(next.graph.root, 'continuation_1')!.state.fields.__ui_data_1.type.items.fields.title
      .type.kind,
    'number'
  );
});

test('output collection item edits update the map and inferred item inputs', () => {
  const document = collectionDocument(),
    original = structuredClone(document);
  const field = nodeOutputFields(findNode(document.graph.root, 'collect')!)[0];
  const next = editOutputField(document, 'collect', field, {
    ...field,
    name: 'topics',
    type: { kind: 'array', items: record({ title: { kind: 'number' } }) },
  });
  assert.equal(
    findNode(next.graph.root, 'each')!.state.fields.__ui_data_1.type.items.fields.title.type.kind,
    'number'
  );
  assert.equal(findNode(next.graph.root, 'draft')!.input.fields.subject.type.kind, 'number');
  assert.deepEqual(findNode(next.graph.root, 'collect')!.writeBindings[0].value.path, ['topics']);
  assert.equal(
    inputSourceLabel(next, findNode(next.graph.root, 'draft')!, 'subject'),
    'Item · title'
  );
  assert.deepEqual(document, original);
});

test('deleting or replacing an output collection unbinds its map and item consumers', () => {
  const document = collectionDocument();
  const field = nodeOutputFields(findNode(document.graph.root, 'collect')!)[0];
  for (const next of [
    removeOutputField(document, 'collect', field),
    editOutputField(document, 'collect', field, { ...field, type: string }),
  ]) {
    assert.equal(findNode(next.graph.root, 'each')!.over, null);
    assert.deepEqual(findNode(next.graph.root, 'draft')!.inputBindings, []);
    assert.equal(findNode(next.graph.root, 'draft')!.input.fields.subject.type.kind, 'string');
    assert.equal(
      inputSourceLabel(next, findNode(next.graph.root, 'draft')!, 'subject'),
      'Choose source'
    );
  }
});

test('removing a referenced collection item field clears only that item source', () => {
  const document = collectionDocument();
  const field = nodeOutputFields(findNode(document.graph.root, 'collect')!)[0];
  const next = editOutputField(document, 'collect', field, {
    ...field,
    type: { kind: 'array', items: record({ description: string }) },
  });
  assert.deepEqual(findNode(next.graph.root, 'each')!.over, {
    source: 'state',
    path: ['__ui_data_1'],
  });
  assert.deepEqual(findNode(next.graph.root, 'draft')!.inputBindings, []);
});

test('a draft map does not present a missing or scalar collection as a selected source', () => {
  const document = collectionDocument();
  const map = findNode(document.graph.root, 'each')!;
  assert.equal(mapSourceLabel(document, map), 'collect · items');
  map.over = { source: 'state', path: ['items'] };
  assert.equal(mapSourceLabel(document, map), 'Choose collection');
  map.state.fields.items = { type: string, required: true };
  assert.equal(mapSourceLabel(document, map), 'Choose collection');
});

test('outcome connections select their existing source without a duplicate saved option', () => {
  const reviewer = {
    ...work('review'),
    kind: 'verifier',
    signals: { verdict: ['accepted', 'rejected'] },
    writeBindings: [
      { target: ['verdict'], value: { node: 'review', channel: 'signal', path: ['verdict'] } },
    ],
  };
  const consumer = {
    ...reader(),
    inputBindings: [{ target: ['verdict'], value: { source: 'state', path: ['verdict'] } }],
  };
  const document = doc(seq('run', [reviewer, consumer]));
  document.graph.root.state.fields.verdict = {
    type: { kind: 'enum', values: ['accepted', 'rejected'] },
    required: true,
  };
  const choices = nodeDataChoices(document, consumer);
  const option = choices.find(
    (choice) => choice.source.kind === 'node_output' && choice.source.channel === 'signal'
  )!;
  assert.equal(inputSourceId(document, consumer, 'verdict', choices), option.id);
});
