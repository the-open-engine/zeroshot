import { allNodes, type Document, type GraphNode } from './domain';

// Presentation helpers for graph.rs Guard, ControlSelector, Join and verifier signals.
// Rust still decides whether references are in scope and conditions are admissible.
export type GuardValue = { kind: string; [key: string]: any };
export type ControlValue = {
  name: string;
  source: string;
  field?: string | null;
  [key: string]: any;
};
export type Signals = Record<string, any>;
export const guardKinds: Record<string, string> = {
  in: 'Label matches',
  all: 'All conditions',
  any: 'Any condition',
  not: 'Not',
  k_of_n: 'At least N results',
  k_of_map: 'At least N map items',
};
export const joinKinds: Record<string, string> = {
  all: 'All branches',
  any: 'Any branch',
  quorum: 'Branch quorum',
  first: 'First matching branch',
};
export const isObject = (value: any): value is Record<string, any> =>
  !!value && typeof value === 'object' && !Array.isArray(value);
export const isStringList = (value: unknown): value is string[] =>
  Array.isArray(value) && value.every((item) => typeof item === 'string');

function knownKind(value: unknown, kinds: Record<string, string>): value is GuardValue {
  return isObject(value) && typeof value.kind === 'string' && Object.hasOwn(kinds, value.kind);
}

function editableControl(value: unknown): value is ControlValue {
  return (
    isObject(value) &&
    typeof value.name === 'string' &&
    typeof value.source === 'string' &&
    ['error', 'signal', 'group'].includes(value.source) &&
    (value.field == null || typeof value.field === 'string')
  );
}

function isPositiveCount(value: unknown): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value > 0;
}

export function controlSources(node?: GraphNode): string[] {
  if (node?.kind === 'verifier') return ['error', 'signal'];
  if (node?.kind === 'step') return ['error'];
  if (node && ['loop', 'map', 'par'].includes(node.kind)) return ['group'];
  return [];
}

export function controlFields(node?: GraphNode): Signals {
  if (node?.kind === 'verifier') return isObject(node.signals) ? node.signals : {};
  if (node?.kind === 'loop') return { terminated: ['converged', 'exhausted'] };
  if (node?.kind === 'map') return { overflow: ['ok', 'overflow'] };
  if (node?.kind === 'par')
    return node.join?.kind === 'first'
      ? { raced: ['satisfied', 'no_satisfier'] }
      : { joined: ['reached', 'quorum_unreachable'] };
  return {};
}

export function controlLabels(document: Document, selector: ControlValue): string[] {
  const node = allNodes(document.graph.root).find((entry) => entry.name === selector.name);
  if (!controlSources(node).includes(selector.source)) return [];
  if (selector.source === 'error')
    return selector.field == null ? ['crash', 'malformed', 'refusal', 'timeout'] : [];
  const labels = controlFields(node)[selector.field ?? ''];
  return isStringList(labels) ? labels : [];
}

export function selectControl(node: GraphNode, source = controlSources(node)[0]): ControlValue {
  return {
    name: node.name,
    source: source ?? 'error',
    field:
      source === 'signal' || source === 'group'
        ? (Object.keys(controlFields(node))[0] ?? '')
        : null,
  };
}

export function freshGuard(
  document: Document,
  owner?: GraphNode,
  candidates?: GraphNode[]
): GuardValue {
  const nodes = candidates ?? allNodes(document.graph.root);
  const node =
    nodes.find(
      (entry) =>
        entry.name !== owner?.name &&
        entry.kind === 'verifier' &&
        Object.keys(controlFields(entry)).length
    ) ??
    nodes.find((entry) => entry.name !== owner?.name && controlSources(entry).length) ??
    nodes.find((entry) => controlSources(entry).length);
  const value = node
    ? selectControl(
        node,
        node.kind === 'verifier' && Object.keys(controlFields(node)).length ? 'signal' : undefined
      )
    : { name: '', source: 'error', field: null };
  return { kind: 'in', value, labels: controlLabels(document, value).slice(0, 1) };
}

export function newGuardKind(
  kind: string,
  document: Document,
  owner?: GraphNode,
  candidates?: GraphNode[]
): GuardValue {
  const leaf = freshGuard(document, owner, candidates);
  if (kind === 'all' || kind === 'any') return { kind, guards: [leaf] };
  if (kind === 'not') return { kind, guard: leaf };
  if (kind === 'k_of_n') return { kind, count: 1, values: [leaf.value], labels: leaf.labels };
  if (kind === 'k_of_map') return { kind, count: 1, value: leaf.value, labels: leaf.labels };
  return leaf;
}

export function wrapGuard(value: GuardValue, kind: string): GuardValue | undefined {
  if (kind === 'all' || kind === 'any')
    return ['all', 'any'].includes(value.kind) && Array.isArray(value.guards)
      ? { ...value, kind }
      : { kind, guards: [value] };
  if (kind === 'not') return { kind, guard: value };
  return undefined;
}

export function editableGuard(value: unknown): value is GuardValue {
  if (!knownKind(value, guardKinds)) return false;
  // Nested conditions render independently, keeping unknown children intact and visible.
  if (value.kind === 'all' || value.kind === 'any') return Array.isArray(value.guards);
  if (value.kind === 'not') return isObject(value.guard);
  if ((value.kind === 'k_of_n' || value.kind === 'k_of_map') && !isPositiveCount(value.count))
    return false;
  const selectors = value.kind === 'k_of_n' ? value.values : [value.value];
  return Array.isArray(selectors) && selectors.every(editableControl) && isStringList(value.labels);
}

export function editableJoin(value: unknown): value is GuardValue {
  if (!knownKind(value, joinKinds)) return false;
  if (value.kind === 'quorum') return isPositiveCount(value.count);
  if (value.kind === 'first') return isObject(value.when);
  return true;
}

export function positiveCount(value: string): number {
  const number = Number(value);
  if (!Number.isSafeInteger(number) || number < 1)
    throw new Error('Enter a whole number of at least 1.');
  return number;
}

export function identifier(value: string): string {
  if (!/^[A-Za-z_][A-Za-z0-9_.-]*$/.test(value) || value.length > 128)
    throw new Error(
      'Use a letter or underscore, then letters, numbers, dots, dashes or underscores (128 maximum).'
    );
  return value;
}

export function addSignal(signals: Signals): Signals {
  let name = Object.hasOwn(signals, 'verdict') ? 'signal' : 'verdict';
  const base = name;
  for (let suffix = 2; Object.hasOwn(signals, name); suffix++) name = `${base}_${suffix}`;
  return { ...signals, [name]: name === 'verdict' ? ['accepted', 'rejected'] : ['value'] };
}

export function renameSignal(signals: Signals, before: string, after: string): Signals {
  identifier(after);
  if (before !== after && Object.hasOwn(signals, after))
    throw new Error('This signal already exists.');
  return Object.fromEntries(
    Object.entries(signals).map(([name, labels]) => [name === before ? after : name, labels])
  );
}

export function changeSignalLabel(
  signals: Signals,
  name: string,
  index: number,
  label: string
): Signals {
  identifier(label);
  const labels = signals[name];
  if (!isStringList(labels)) throw new Error('This signal declaration is not editable.');
  if (labels.some((existing, i) => i !== index && existing === label))
    throw new Error('This label already exists.');
  return { ...signals, [name]: labels.map((existing, i) => (i === index ? label : existing)) };
}

export function appendSignalLabel(signals: Signals, name: string): Signals {
  const labels = signals[name];
  if (!isStringList(labels)) throw new Error('This signal declaration is not editable.');
  let label = 'label';
  for (let suffix = 2; labels.includes(label); suffix++) label = `label_${suffix}`;
  return { ...signals, [name]: [...labels, label] };
}
