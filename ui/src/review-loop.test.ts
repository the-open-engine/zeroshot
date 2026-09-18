import test from 'node:test';
import assert from 'node:assert/strict';
import { createReviewLoop } from './review-loop';
import { allNodes, findNode, pathTo, type Document, type GraphNode } from './domain';

const text = { kind: 'string' };
const field = (type = text, required = true) => ({ type, required });
const object = (fields: Record<string, any> = {}) => ({ kind: 'record', fields });
const state = () => object({ task: field(), result: field(text, false) });
const input = () => object({ task: field() });
const worker = (): GraphNode => ({
  kind: 'step',
  name: 'draft',
  worker: 'authored.writer@1',
  instructions: 'Prepare a release note.',
  input: input(),
  output: object({ result: field() }),
  inputBindings: [
    { target: ['task'], value: { source: 'state', path: ['task'] }, metadata: 'input' },
  ],
  writeBindings: [
    {
      target: ['result'],
      value: { node: 'draft', channel: 'out', path: ['result'] },
      metadata: 'output',
    },
  ],
  attempts: 1,
  timeoutMs: 4000,
  metadata: { retain: true },
});
const done = (): GraphNode => ({
  kind: 'succeed',
  name: 'done',
  output: object({ result: field() }),
  bindings: [{ target: ['result'], value: { source: 'state', path: ['result'] } }],
});
const sequence = (name: string, children: GraphNode[], schema = state()): GraphNode => ({
  kind: 'seq',
  name,
  state: schema,
  children,
  promotedStatePaths: [],
});
function document(root = sequence('run', [worker(), done()])): Document {
  return {
    name: 'review-example',
    graph: {
      profile: 'openengine.graph.full/v1',
      initialInput: input(),
      policy: { policy: 'policy.native-v2@1', default: 'deny' },
      root,
    },
    runtime: {
      harness: 'codex',
      provider: 'openai',
      size: 'small',
      nodes: {
        draft: {
          kind: 'agent',
          model: 'caller-model',
          effort: 'high',
          sessionScope: 'node',
          connections: { tools: ['PRIVATE_TOKEN'] },
        },
      },
    },
  };
}
const fullError = {
  kind: 'in',
  value: { name: 'draft', source: 'error', field: null },
  labels: ['timeout', 'refusal', 'crash', 'malformed'],
};

test('review loop keeps writer identity, runtime and existing data while wiring feedback and review results', () => {
  const source = document(),
    before = structuredClone(source);
  const result = createReviewLoop(source, 'draft');
  const loop = findNode(result.document.graph.root, result.name)!;
  const work = findNode(result.document.graph.root, 'draft')!;
  const reviewer = allNodes(loop).find((node) => node.kind === 'verifier')!;
  assert.equal(loop.kind, 'loop');
  assert.equal(loop.maxIterations, 3);
  assert.equal(work.worker, before.graph.root.children[0].worker);
  assert.equal(work.kind, 'step');
  assert.equal(work.timeoutMs, 4000);
  assert.deepEqual(work.metadata, { retain: true });
  assert.deepEqual(work.output, before.graph.root.children[0].output);
  assert.deepEqual(work.inputBindings[0], before.graph.root.children[0].inputBindings[0]);
  assert.deepEqual(work.writeBindings, before.graph.root.children[0].writeBindings);
  assert.deepEqual(result.document.runtime.nodes.draft, before.runtime.nodes.draft);
  assert.deepEqual(result.document.runtime.nodes[reviewer.name], {
    kind: 'agent',
    model: 'caller-model',
    effort: 'high',
  });
  assert.notEqual(reviewer.worker, work.worker);
  assert.deepEqual(reviewer.signals, { verdict: ['accepted', 'rejected'] });
  assert.equal(reviewer.diagnostic.fields.feedback.type.kind, 'string');
  assert.equal(
    reviewer.inputBindings.find((binding: any) => binding.target[0] === 'result').value.path[0],
    'result'
  );
  const feedback = reviewer.writeBindings[0].target[0];
  assert.equal(result.document.graph.root.state.fields[feedback].required, true);
  assert.equal(result.document.graph.root.state.fields[feedback].type.kind, 'string');
  assert.ok(
    work.inputBindings.some(
      (binding: any) => binding.target[0] === feedback && binding.value.path[0] === feedback
    )
  );
  assert.match(work.instructions, /empty on the first attempt/);
  assert.deepEqual(result.document.graph.initialInput, before.graph.initialInput);
  assert.deepEqual(source, before);
});

test('writer error stops before the reviewer; reviewer errors and exhausted budgets cannot succeed', () => {
  const result = createReviewLoop(document(), 'draft');
  const loop = findNode(result.document.graph.root, result.name)!;
  const checkpoint = loop.body.children[1];
  assert.equal(checkpoint.branches[0].node.kind, 'fail');
  assert.deepEqual(new Set(checkpoint.branches[0].when.labels), new Set(fullError.labels));
  assert.equal(checkpoint.branches[0].when.value.name, 'draft');
  const reviewer = checkpoint.otherwise;
  assert.equal(reviewer.kind, 'verifier');
  assert.deepEqual(loop.until.guards[0], {
    kind: 'in',
    value: { name: reviewer.name, source: 'signal', field: 'verdict' },
    labels: ['accepted'],
  });
  assert.equal(loop.until.guards[1].value.name, reviewer.name);
  const outcome = result.document.graph.root.children[1];
  assert.equal(outcome.branches[0].when.value.name, reviewer.name);
  assert.equal(outcome.branches[0].node.reason, 'review_failed');
  assert.equal(outcome.branches[1].when.value.name, loop.name);
  assert.deepEqual(outcome.branches[1].when.labels, ['exhausted']);
  assert.equal(outcome.branches[1].node.reason, 'review_exhausted');
  assert.deepEqual(outcome.otherwise, done());
  assert.equal(allNodes(loop).filter((node) => node.kind === 'succeed').length, 0);
});

test('an ordinary native error checkpoint moves into the loop while its original continuation is retained', () => {
  const failure = { kind: 'fail', name: 'original_failure', reason: 'drafting_failed' };
  const checkpoint: GraphNode = {
    kind: 'choice',
    name: 'draft_result',
    state: state(),
    branches: [{ when: fullError, node: failure }],
    otherwise: done(),
    promotedStatePaths: [],
    metadata: { context: 'preserve' },
  };
  const source = document(sequence('run', [worker(), checkpoint]));
  const result = createReviewLoop(source, 'draft');
  const moved = findNode(result.document.graph.root, 'draft_result')!;
  assert.equal(moved.otherwise.kind, 'verifier');
  assert.deepEqual(moved.branches[0].node, failure);
  assert.deepEqual(moved.metadata, checkpoint.metadata);
  assert.deepEqual(findNode(result.document.graph.root, 'done'), done());
  assert.equal(
    allNodes(result.document.graph.root).filter((node) => node.name === 'draft_result').length,
    1
  );
});

test('root or final root-sequence writers gain a structured completion without changing caller input', () => {
  for (const root of [worker(), sequence('run', [worker()])]) {
    const source = document(root);
    if (root.kind === 'step') {
      source.graph.initialInput = state();
    }
    const before = structuredClone(source);
    const result = createReviewLoop(source, 'draft');
    const terminal = allNodes(result.document.graph.root).find((node) => node.kind === 'succeed')!;
    assert.deepEqual(terminal.output, worker().output);
    assert.deepEqual(terminal.bindings, [
      { target: ['result'], value: { source: 'state', path: ['result'] } },
    ]);
    assert.deepEqual(result.document.graph.initialInput, before.graph.initialInput);
    assert.deepEqual(source, before);
  }
});

test('unmapped required outputs get dedicated optional state paths; prior mappings are never replaced', () => {
  const source = document(sequence('run', [worker()]));
  source.graph.root.children[0].writeBindings = [];
  const result = createReviewLoop(source, 'draft');
  const work = findNode(result.document.graph.root, 'draft')!;
  const mapping = work.writeBindings[0];
  assert.deepEqual(mapping.value, { node: 'draft', channel: 'out', path: ['result'] });
  assert.notEqual(mapping.target[0], 'result');
  assert.equal(result.document.graph.root.state.fields[mapping.target[0]].required, false);
  const terminal = allNodes(result.document.graph.root).find((node) => node.kind === 'succeed')!;
  assert.deepEqual(terminal.bindings[0].value.path, mapping.target);
});

test('nested sequences retain their parent position and forward suffix with an independent feedback field', () => {
  const inner = sequence('draft_sequence', [worker(), done()]);
  const source = document(sequence('run', [inner]));
  const result = createReviewLoop(source, 'draft');
  assert.equal(result.document.graph.root.children[0].name, 'draft_sequence');
  assert.equal(pathTo(result.document.graph.root, result.name).at(-2)!.name, 'draft_sequence');
  assert.equal(findNode(result.document.graph.root, 'done')!.kind, 'succeed');
  const reviewer = allNodes(result.document.graph.root).find((node) => node.kind === 'verifier')!;
  const feedback = reviewer.writeBindings[0].target[0];
  for (const ancestor of pathTo(result.document.graph.root, 'draft').slice(0, -1))
    assert.equal(ancestor.state.fields[feedback].type.kind, 'string');
});

test('multi-activity suffix remains ordered and shares its original runtime settings', () => {
  const second = {
    ...worker(),
    name: 'publish_draft',
    worker: 'authored.publisher@1',
    writeBindings: [],
  };
  const source = document(sequence('run', [worker(), second, done()]));
  source.runtime.nodes.publish_draft = { kind: 'agent', model: 'another-model' };
  const result = createReviewLoop(source, 'draft');
  const continuation = result.document.graph.root.children[1].otherwise;
  assert.deepEqual(
    continuation.children.map((node: GraphNode) => node.name),
    ['publish_draft', 'done']
  );
  assert.deepEqual(findNode(result.document.graph.root, 'publish_draft'), second);
  assert.deepEqual(result.document.runtime.nodes.publish_draft, source.runtime.nodes.publish_draft);
});

test('new identities and feedback fields avoid authored and dangling references', () => {
  const source = document();
  source.graph.root.state.fields.reviewFeedback = field();
  source.graph.root.children[0].input.fields.reviewFeedback = field();
  source.graph.root.children[0].writeBindings.push({
    target: ['result'],
    value: { node: 'draft_reviewer', channel: 'out', path: ['result'] },
  });
  const result = createReviewLoop(source, 'draft');
  const reviewer = allNodes(result.document.graph.root).find((node) => node.kind === 'verifier')!;
  assert.notEqual(reviewer.name, 'draft_reviewer');
  assert.notEqual(reviewer.writeBindings[0].target[0], 'reviewFeedback');
  assert.deepEqual(
    findNode(result.document.graph.root, 'draft')!.writeBindings,
    source.graph.root.children[0].writeBindings
  );
});

test('None inputs and outputs remain useful for workspace work with no fabricated result fields', () => {
  const source = document(sequence('run', [worker()]));
  Object.assign(source.graph.root.children[0], {
    input: { kind: 'null' },
    output: { kind: 'null' },
    inputBindings: [],
    writeBindings: [],
  });
  const result = createReviewLoop(source, 'draft');
  const work = findNode(result.document.graph.root, 'draft')!;
  assert.equal(work.kind, 'step');
  assert.equal(work.input.kind, 'record');
  assert.equal(Object.keys(work.input.fields).length, 1);
  const terminal = allNodes(result.document.graph.root).find((node) => node.kind === 'succeed')!;
  assert.deepEqual(terminal.output, { kind: 'null' });
  assert.deepEqual(terminal.bindings, []);
});

test('unsupported contexts fail atomically without coercing workers or schemas', () => {
  const cases: [Document, RegExp][] = [];
  const verifier = document();
  verifier.graph.root.children[0].kind = 'verifier';
  cases.push([verifier, /writing Agent/]);
  const delivery = document();
  delivery.runtime.nodes.draft.kind = 'git_delivery';
  cases.push([delivery, /writing Agent/]);
  const scalar = document();
  scalar.graph.initialInput = { kind: 'null' };
  cases.push([scalar, /Object run inputs/]);
  const optional = document();
  optional.graph.root.children[0].output.fields.result.required = false;
  cases.push([optional, /output fields required/]);
  const nestedLast = document(sequence('run', [sequence('inner', [worker()])]));
  cases.push([nestedLast, /following activity/]);
  for (const [source, message] of cases) {
    const before = structuredClone(source);
    assert.throws(() => createReviewLoop(source, 'draft'), message);
    assert.deepEqual(source, before);
  }
});
