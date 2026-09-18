// A review loop is an authored native graph, including feedback and failure paths.
// It never changes worker identity, invents protocol controls, or starts a run.
import {
  assertDocument,
  clone,
  findNode,
  isGroup,
  pathTo,
  uniqueAgentReference,
  uniqueName,
  type Document,
  type GraphNode,
} from './domain';
import { bindingKey, payloadAtPath } from './bindings';
import { recordFields, type Payload } from './schema';

const errors = ['crash', 'malformed', 'refusal', 'timeout'];
const string = { kind: 'string' };
const record = (fields: Record<string, any> = {}): Payload => ({ kind: 'record', fields });
const validPath = (path: any): path is string[] =>
  Array.isArray(path) && path.length > 0 && path.every((part) => typeof part === 'string');
const errorGuard = (name: string) => ({
  kind: 'in',
  value: { name, source: 'error', field: null },
  labels: [...errors],
});
const promotes = (nodes: GraphNode[]): string[][] =>
  uniquePaths(
    nodes.flatMap((node) =>
      isGroup(node)
        ? (node.promotedStatePaths ?? [])
        : ['step', 'verifier'].includes(node.kind)
          ? (node.writeBindings ?? []).map((binding: any) => binding.target)
          : []
    )
  );
const prefix = (left: string[], right: string[]) =>
  left.length <= right.length && left.every((part, index) => part === right[index]);

function uniquePaths(paths: any[]): string[][] {
  if (paths.some((path) => !validPath(path)))
    throw new Error('Repair the activity’s unsupported state writes before adding a review loop.');
  const unique = [...new Map(paths.map((path) => [bindingKey(path), clone(path)])).values()];
  return unique.filter((path) => !unique.some((other) => other !== path && prefix(other, path)));
}

function names(document: Document) {
  const reserved = new Set<string>();
  return (hint: string) => {
    const base = hint.slice(0, 96);
    let candidate = uniqueName(document.graph.root, base);
    for (let suffix = 2; reserved.has(candidate); suffix++)
      candidate = uniqueName(document.graph.root, `${base}_${suffix}`);
    reserved.add(candidate);
    return candidate;
  };
}

function fields(document: Document) {
  const reserved = new Set<string>();
  const pending: any[] = [document.graph];
  while (pending.length) {
    const value = pending.pop();
    if (!value || typeof value !== 'object') continue;
    for (const name of Object.keys(recordFields(value) ?? {})) reserved.add(name);
    if (validPath(value.path)) value.path.forEach((name: string) => reserved.add(name));
    if (validPath(value.target)) value.target.forEach((name: string) => reserved.add(name));
    for (const child of Object.values(value))
      if (child && typeof child === 'object') pending.push(child);
  }
  return (hint: string) => {
    const base = hint.slice(0, 96);
    let candidate = base;
    for (let suffix = 2; reserved.has(candidate); suffix++) candidate = `${base}_${suffix}`;
    reserved.add(candidate);
    return candidate;
  };
}

function stateOf(node: GraphNode): Payload {
  if (!recordFields(node.state))
    throw new Error(
      'Use Object state in this sequence and its ancestors before adding a review loop.'
    );
  return node.state;
}

function plainErrorCheckpoint(node: GraphNode | undefined, activity: string, state: Payload) {
  if (node?.kind !== 'choice' || node.branches?.length !== 1 || !node.otherwise) return false;
  if (
    JSON.stringify(node.state) !== JSON.stringify(state) ||
    !Array.isArray(node.promotedStatePaths ?? []) ||
    (node.promotedStatePaths ?? []).length
  )
    return false;
  const branch = node.branches[0],
    guard = branch.when;
  return (
    branch.node?.kind === 'fail' &&
    guard?.kind === 'in' &&
    guard.value?.name === activity &&
    guard.value?.source === 'error' &&
    guard.value?.field == null &&
    Array.isArray(guard.labels) &&
    guard.labels.length === errors.length &&
    errors.every((label) => guard.labels.includes(label))
  );
}

export function createReviewLoop(
  document: Document,
  activityName: string
): { document: Document; name: string } {
  assertDocument(document);
  const selected = findNode(document.graph.root, activityName);
  if (
    !selected ||
    selected.kind !== 'step' ||
    document.runtime.nodes[activityName]?.kind !== 'agent'
  )
    throw new Error('Choose a writing Agent to repeat until a review approves its work.');
  const ancestry = pathTo(document.graph.root, activityName);
  const parent = ancestry.at(-2);
  if (parent && parent.kind !== 'seq')
    throw new Error('Place this activity in a Sequence before adding its review loop.');
  if (ancestry.slice(0, -1).some((node) => !['seq', 'choice'].includes(node.kind)))
    throw new Error('Add this review loop outside existing loops, maps, or parallel branches.');
  if (!recordFields(document.graph.initialInput))
    throw new Error('Use Object run inputs before adding a review loop with automatic feedback.');
  if (!recordFields(selected.input) && selected.input?.kind !== 'null')
    throw new Error('Use an Object or None activity input before adding review feedback.');
  const output = recordFields(selected.output);
  if (!output && selected.output?.kind !== 'null')
    throw new Error(
      'Use an Object or None activity output so the reviewer can receive its result.'
    );
  if (Object.values(output ?? {}).some((field: any) => field.required !== true))
    throw new Error('Make the activity’s output fields required before adding automatic review.');
  for (const key of ['inputBindings', 'writeBindings'])
    if (!Array.isArray(selected[key] ?? []))
      throw new Error('Repair the activity’s unsupported mappings before adding a review loop.');
  const originalWrites = uniquePaths(
    (selected.writeBindings ?? []).map((binding: any) => binding.target)
  );
  for (const ancestor of ancestry.slice(0, -1)) stateOf(ancestor);
  const at = parent?.children.findIndex((node: GraphNode) => node.name === activityName) ?? 0;
  const after = parent ? parent.children.slice(at + 1) : [];
  if (parent && parent.name !== document.graph.root.name && !after.length)
    throw new Error(
      'Add this review loop where its sequence has a following activity or completion.'
    );

  const next = clone(document);
  const nodeName = names(document),
    fieldName = fields(document);
  const loopName = nodeName(`${activityName}_review`);
  const reviewerName = nodeName(`${activityName}_reviewer`);
  const feedback = fieldName('reviewFeedback');
  const work = clone(selected);
  const nextAncestors = pathTo(next.graph.root, activityName).slice(0, -1);
  let sequence = nextAncestors.at(-1);
  if (!sequence) {
    sequence = {
      kind: 'seq',
      name: nodeName(`${activityName}_workflow`),
      state: clone(document.graph.initialInput),
      children: [clone(selected)],
      promotedStatePaths: [],
    };
    next.graph.root = sequence;
    nextAncestors.push(sequence);
  }
  // Root-owned deterministic empty strings provide no review feedback on round one.
  for (const ancestor of nextAncestors)
    ancestor.state = {
      ...ancestor.state,
      fields: { ...stateOf(ancestor).fields, [feedback]: { type: clone(string), required: true } },
    };
  work.input = {
    ...(recordFields(work.input) ? work.input : record()),
    fields: { ...recordFields(work.input), [feedback]: { type: clone(string), required: true } },
  };
  work.inputBindings = [
    ...(work.inputBindings ?? []),
    { target: [feedback], value: { source: 'state', path: [feedback] } },
  ];
  work.writeBindings = clone(work.writeBindings ?? []);
  work.instructions = `${work.instructions ?? 'Complete the assigned task.'}\n\nReview feedback arrives in input.${feedback}. It is empty on the first attempt. On later attempts, address that feedback while preserving the original requirements; do not weaken the acceptance criteria.`;

  const reviewerInput = clone(work.input);
  const reviewerInputs = clone(work.inputBindings);
  const resultBindings: any[] = [];
  const reviewerFieldNames = new Set(Object.keys(reviewerInput.fields));
  for (const [field, definition] of Object.entries(output ?? {})) {
    const existing = work.writeBindings.find(
      (binding: any) =>
        binding.value?.node === activityName &&
        binding.value?.channel === 'out' &&
        bindingKey(binding.value.path) === bindingKey([field]) &&
        payloadAtPath(sequence.state, binding.target)
    );
    const target = existing?.target ?? [fieldName(`${activityName}_${field}_result`)];
    if (!existing) {
      sequence.state = {
        ...sequence.state,
        fields: {
          ...sequence.state.fields,
          [target[0]]: { type: clone(definition.type), required: false },
        },
      };
      work.writeBindings.push({
        target,
        value: { node: activityName, channel: 'out', path: [field] },
      });
    }
    let inputField = field;
    for (let suffix = 1; reviewerFieldNames.has(inputField); suffix++)
      inputField = `${field.slice(0, 96)}_result${suffix === 1 ? '' : `_${suffix}`}`;
    reviewerFieldNames.add(inputField);
    reviewerInput.fields = { ...reviewerInput.fields, [inputField]: clone(definition) };
    reviewerInputs.push({ target: [inputField], value: { source: 'state', path: clone(target) } });
    resultBindings.push({ target: [field], value: { source: 'state', path: clone(target) } });
  }
  const state = clone(sequence.state);
  const feedbackPath = [feedback];
  const exposed = uniquePaths([
    ...originalWrites,
    ...work.writeBindings.map((binding: any) => binding.target),
    feedbackPath,
  ]);
  const fail = (name: string, reason: string): GraphNode => ({ kind: 'fail', name, reason });
  const reviewer: GraphNode = {
    kind: 'verifier',
    name: reviewerName,
    worker: uniqueAgentReference(next.graph.root, reviewerName),
    input: reviewerInput,
    output: { kind: 'null' },
    inputBindings: reviewerInputs,
    diagnostic: record({ feedback: { type: clone(string), required: true } }),
    signals: { verdict: ['accepted', 'rejected'] },
    writeBindings: [
      {
        target: feedbackPath,
        value: { node: reviewerName, channel: 'diagnostic', path: ['feedback'] },
      },
    ],
    attempts: 1,
    instructions: `Independently review ${activityName}'s work against the original request and supplied inputs. Inspect its structured results and relevant workspace files and observable checks. The assignment under review is:\n\n${selected.instructions ?? 'Complete the assigned task.'}\n\nReturn verdict accepted only when the requested result is correct, complete, and supported by evidence. Otherwise return rejected with concise, actionable feedback naming the remaining defects. Feedback may be empty when accepted. Review only: do not modify files, use git commands, or perform delivery actions.`,
  };
  const checkpoint = after[0];
  const reuse = plainErrorCheckpoint(
    checkpoint,
    activityName,
    parent?.state ?? document.graph.initialInput
  );
  const suffix: GraphNode[] = clone(reuse ? [checkpoint.otherwise, ...after.slice(1)] : after);
  const writerResult: GraphNode = {
    ...(reuse ? clone(checkpoint) : {}),
    kind: 'choice',
    name: reuse ? checkpoint.name : nodeName(`${activityName}_execution`),
    state: clone(state),
    branches: [
      {
        ...(reuse ? clone(checkpoint.branches[0]) : {}),
        when: errorGuard(activityName),
        node: reuse
          ? clone(checkpoint.branches[0].node)
          : fail(nodeName(`${activityName}_failed`), 'activity_failed'),
      },
    ],
    otherwise: reviewer,
    promotedStatePaths: [feedbackPath],
  };
  const loop: GraphNode = {
    kind: 'loop',
    name: loopName,
    state: clone(state),
    maxIterations: 3,
    until: {
      kind: 'any',
      guards: [
        {
          kind: 'in',
          value: { name: reviewerName, source: 'signal', field: 'verdict' },
          labels: ['accepted'],
        },
        errorGuard(reviewerName),
      ],
    },
    body: {
      kind: 'seq',
      name: nodeName(`${activityName}_review_round`),
      state: clone(state),
      children: [work, writerResult],
      promotedStatePaths: exposed,
    },
    promotedStatePaths: exposed,
  };
  const parentPromotions = uniquePaths(sequence.promotedStatePaths ?? []);
  const suffixWrites = promotes(suffix);
  const returned = parentPromotions.filter((path) =>
    suffixWrites.some((write) => prefix(path, write) || prefix(write, path))
  );
  const continuation: GraphNode =
    suffix.length === 1
      ? suffix[0]
      : suffix.length
        ? {
            kind: 'seq',
            name: nodeName(`${activityName}_after_review`),
            state: clone(state),
            children: suffix,
            promotedStatePaths: returned,
          }
        : {
            kind: 'succeed',
            name: nodeName(`${activityName}_approved`),
            output: clone(selected.output),
            bindings: resultBindings,
          };
  const outcome: GraphNode = {
    kind: 'choice',
    name: nodeName(`${activityName}_review_result`),
    state: clone(state),
    branches: [
      {
        when: errorGuard(reviewerName),
        node: fail(nodeName(`${activityName}_review_failed`), 'review_failed'),
      },
      {
        when: {
          kind: 'in',
          value: { name: loopName, source: 'group', field: 'terminated' },
          labels: ['exhausted'],
        },
        node: fail(nodeName(`${activityName}_review_exhausted`), 'review_exhausted'),
      },
    ],
    otherwise: continuation,
    promotedStatePaths: returned,
  };
  sequence.children.splice(at, sequence.children.length - at, loop, outcome);
  const originalRuntime = document.runtime.nodes[activityName];
  next.runtime.nodes[reviewerName] = {
    kind: 'agent',
    model: originalRuntime.model ?? '',
    ...(originalRuntime.effort ? { effort: originalRuntime.effort } : {}),
  };
  assertDocument(next);
  return { document: next, name: loopName };
}
