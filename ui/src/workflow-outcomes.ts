import type { Document, GraphNode } from './domain';
import { isObject, isStringList, type GuardValue } from './guards';

/** An authored failure route, presented as a setting at its original checkpoint. */
export type WorkflowFailurePolicy = {
  owner: string;
  choiceName: string;
  checkpoint: string;
  branchIndex: number;
  failName: string;
  reason: string;
  guard: GuardValue;
  sources: string[];
  kind: 'worker-error' | 'control-limit' | 'mixed-error';
  boundary: 'run' | 'parallel-branch';
  parallelName?: string;
};
export type WorkflowCompletion = {
  name: string;
  owner: string;
  scope: string;
  mode: 'final' | 'early';
  boundary: 'run' | 'parallel-branch';
  parallelName?: string;
};
export type WorkflowOutcomes = {
  policies: WorkflowFailurePolicy[];
  completions: WorkflowCompletion[];
};

type Location = { node: GraphNode; ancestors: GraphNode[]; previous?: GraphNode };
type RecognizedGuard = { sources: string[]; kinds: Set<'worker-error' | 'control-limit'> };
type Route = { node: GraphNode; predicates: unknown[] };
const errors = new Set(['timeout', 'crash', 'malformed', 'refusal']);
const onlyKeys = (value: Record<string, unknown>, keys: string[]) =>
  Object.keys(value).every((key) => keys.includes(key));
const nodeLike = (value: unknown): value is GraphNode =>
  isObject(value) && typeof value.kind === 'string' && typeof value.name === 'string';
const list = (value: unknown): any[] => (Array.isArray(value) ? value : []);

function children(node: GraphNode): GraphNode[] {
  if (node.kind === 'seq') return list(node.children).filter(nodeLike);
  if (node.kind === 'par') return list(node.branches).filter(nodeLike);
  if (node.kind === 'choice')
    return [
      ...list(node.branches)
        .map((branch) => branch?.node)
        .filter(nodeLike),
      ...(nodeLike(node.otherwise) ? [node.otherwise] : []),
    ];
  if (node.kind === 'loop' || node.kind === 'map') return nodeLike(node.body) ? [node.body] : [];
  return [];
}

type FailureLeaf = Record<string, unknown> & {
  value: Record<string, unknown> & { name: string };
  labels: string[];
};

function validFailureLeaf(guard: Record<string, unknown>): guard is FailureLeaf {
  return (
    ['in', 'k_of_map'].includes(guard.kind as string) &&
    onlyKeys(
      guard,
      guard.kind === 'in' ? ['kind', 'value', 'labels'] : ['kind', 'count', 'value', 'labels']
    ) &&
    isObject(guard.value) &&
    onlyKeys(guard.value, ['name', 'source', 'field']) &&
    typeof guard.value.name === 'string' &&
    !!guard.value.name &&
    isStringList(guard.labels) &&
    guard.labels.length > 0
  );
}

function expectedControl(node: GraphNode): [string, string] | undefined {
  if (node.kind === 'loop') return ['terminated', 'exhausted'];
  if (node.kind === 'map') return ['overflow', 'overflow'];
  if (node.kind !== 'par') return undefined;
  if (node.join?.kind === 'first') return ['raced', 'no_satisfier'];
  return ['all', 'any', 'quorum'].includes(node.join?.kind)
    ? ['joined', 'quorum_unreachable']
    : undefined;
}

function validMapCount(guard: FailureLeaf, location: Location): boolean {
  return (
    guard.kind !== 'k_of_map' ||
    (Number.isSafeInteger(guard.count) &&
      (guard.count as number) >= 1 &&
      location.ancestors.some((node) => node.kind === 'map'))
  );
}

function matchesWorkerError(guard: FailureLeaf, location: Location): boolean {
  return (
    guard.value.source === 'error' &&
    guard.value.field == null &&
    ['step', 'verifier'].includes(location.node.kind) &&
    guard.labels.length === errors.size &&
    new Set(guard.labels).size === errors.size &&
    guard.labels.every((label) => errors.has(label))
  );
}

/**
 * Recognize only complete, known error/limit guards. This does not prove guard
 * availability or alter its evaluation; native verification still owns both.
 */
function failureGuard(
  guard: unknown,
  locations: Map<string, Location | undefined>,
  depth = 0
): RecognizedGuard | undefined {
  if (!isObject(guard) || depth > 64) return undefined;
  if (guard.kind === 'any') {
    if (
      !onlyKeys(guard, ['kind', 'guards']) ||
      !Array.isArray(guard.guards) ||
      !guard.guards.length
    )
      return undefined;
    const parts = guard.guards.map((part) => failureGuard(part, locations, depth + 1));
    if (parts.some((part) => !part)) return undefined;
    return {
      sources: [...new Set(parts.flatMap((part) => part!.sources))],
      kinds: new Set(parts.flatMap((part) => [...part!.kinds])),
    };
  }
  if (!validFailureLeaf(guard)) return undefined;
  const location = locations.get(guard.value.name);
  if (!location) return undefined;
  if (!validMapCount(guard, location)) return undefined;
  if (matchesWorkerError(guard, location))
    return { sources: [location.node.name], kinds: new Set(['worker-error']) };
  if (guard.kind !== 'in' || guard.value.source !== 'group' || guard.labels.length !== 1)
    return undefined;
  const { node } = location;
  const expected = expectedControl(node);
  return expected && guard.value.field === expected[0] && guard.labels[0] === expected[1]
    ? { sources: [node.name], kinds: new Set(['control-limit']) }
    : undefined;
}

function matchingRouteSources(
  route: Route,
  term: unknown,
  locations: Map<string, Location | undefined>,
  canonical: (value: any) => string
): string[] | undefined {
  if (!isObject(term) || term.kind !== 'all' || !onlyKeys(term, ['kind', 'guards']))
    return undefined;
  const parts = list(term.guards);
  if (
    parts.length !== route.predicates.length + 1 ||
    route.predicates.some((predicate, i) => canonical(predicate) !== canonical(parts[i]))
  )
    return undefined;
  const errors = failureGuard(parts.at(-1), locations);
  if (
    !errors ||
    errors.kinds.size !== 1 ||
    !errors.kinds.has('worker-error') ||
    errors.sources.some((name) => {
      const source = locations.get(name);
      return !source || (source.node !== route.node && !source.ancestors.includes(route.node));
    })
  )
    return undefined;
  return errors.sources;
}

// Match the native Choice protection lowering exactly. Route predicates must
// precede the error test, so controls from an unselected branch stay untouched.
function choiceFailureGuard(
  guard: unknown,
  choice: GraphNode | undefined,
  locations: Map<string, Location | undefined>
): RecognizedGuard | undefined {
  if (choice?.kind !== 'choice' || !isObject(guard)) return undefined;
  const continues = (node: GraphNode): boolean =>
    !['fail', 'succeed'].includes(node.kind) &&
    (node.kind === 'seq' ? children(node).every(continues) : true);
  const canonical = (value: any): string =>
    JSON.stringify(value, (_, entry) =>
      isObject(entry)
        ? Object.fromEntries(Object.entries(entry).sort(([a], [b]) => a.localeCompare(b)))
        : entry
    );
  const routes: Route[] = [];
  const prior: unknown[] = [];
  for (const branch of list(choice.branches)) {
    if (!nodeLike(branch?.node) || !isObject(branch.when)) return undefined;
    if (continues(branch.node))
      routes.push({ node: branch.node, predicates: [...prior, branch.when] });
    prior.push({ kind: 'not', guard: branch.when });
  }
  if (nodeLike(choice.otherwise) && continues(choice.otherwise))
    routes.push({ node: choice.otherwise, predicates: prior });
  const terms =
    guard.kind === 'any' && onlyKeys(guard, ['kind', 'guards']) ? list(guard.guards) : [guard];
  if (!routes.length || terms.length !== routes.length) return undefined;
  const sources: string[] = [];
  for (const [index, route] of routes.entries()) {
    const matched = matchingRouteSources(route, terms[index], locations, canonical);
    if (!matched) return undefined;
    sources.push(...matched);
  }
  return { sources: [...new Set(sources)], kinds: new Set(['worker-error']) };
}

/** Lossless presentation metadata. Returned guards are the original authored values. */
export function analyzeWorkflowOutcomes(document: Document): WorkflowOutcomes {
  const result: WorkflowOutcomes = { policies: [], completions: [] };
  const entries: Location[] = [];
  const locations = new Map<string, Location | undefined>();
  function collect(node: GraphNode, ancestors: GraphNode[], previous?: GraphNode) {
    if (ancestors.includes(node) || ancestors.length > 64) return;
    const location = { node, ancestors, previous };
    entries.push(location);
    // Duplicate authored names cannot acquire a guessed presentation owner.
    locations.set(node.name, locations.has(node.name) ? undefined : location);
    children(node).forEach((child, index, siblings) =>
      collect(child, [...ancestors, node], node.kind === 'seq' ? siblings[index - 1] : undefined)
    );
  }
  collect(document.graph.root, []);
  for (const location of entries) {
    const { node, ancestors, previous } = location;
    if (locations.get(node.name) !== location) continue;
    const parallel = [...ancestors].reverse().find((ancestor) => ancestor.kind === 'par');
    const boundary = parallel
      ? { boundary: 'parallel-branch' as const, parallelName: parallel.name }
      : { boundary: 'run' as const };
    if (node.kind === 'choice') {
      list(node.branches).forEach((branch, branchIndex) => {
        if (
          !isObject(branch) ||
          !onlyKeys(branch, ['when', 'node']) ||
          !nodeLike(branch.node) ||
          branch.node.kind !== 'fail' ||
          !onlyKeys(branch.node, ['kind', 'name', 'reason']) ||
          typeof branch.node.reason !== 'string' ||
          locations.get(branch.node.name)?.node !== branch.node
        )
          return;
        const recognized =
          failureGuard(branch.when, locations) ??
          choiceFailureGuard(branch.when, previous, locations);
        if (!recognized) return;
        // Association means AFTER this exact preceding worker/control settles.
        // Never hoist a branch-local failure to an enclosing parallel group.
        const owner =
          previous &&
          recognized.sources.every((name) => {
            const source = locations.get(name)!;
            return source.node === previous || source.ancestors.includes(previous);
          })
            ? previous.name
            : node.name;
        result.policies.push({
          owner,
          choiceName: node.name,
          checkpoint: node.name,
          branchIndex,
          failName: branch.node.name,
          reason: branch.node.reason,
          guard: branch.when as GuardValue,
          sources: recognized.sources,
          kind: recognized.kinds.size === 1 ? [...recognized.kinds][0] : 'mixed-error',
          ...boundary,
        });
      });
    }
    if (node.kind === 'succeed') {
      const path = [...ancestors, node];
      const final = ancestors.every((ancestor, index) => {
        if (['loop', 'map', 'par'].includes(ancestor.kind)) return false;
        if (ancestor.kind === 'seq') return children(ancestor).at(-1) === path[index + 1];
        return ancestor.kind === 'choice';
      });
      const scope = ancestors.at(-1)?.name ?? node.name;
      result.completions.push({
        name: node.name,
        owner: previous?.name ?? scope,
        scope,
        mode: final ? 'final' : 'early',
        ...boundary,
      });
    }
  }
  return result;
}
