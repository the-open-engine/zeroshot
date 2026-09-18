// Scope-preserving workflow actions. These edit the structured AST; they never infer a
// successful branch, flatten execution semantics, or manufacture map item/state schemas.
import { analyzeWorkflowOutcomes } from './workflow-outcomes';
import {
  addNode,
  assertDocument,
  bindingFor,
  children,
  clone,
  executable,
  findNode,
  isGroup,
  pathTo,
  uniqueName,
  type Document,
  type GraphNode,
} from './domain';
import { editableActivityRole } from './activity-access';

export type WorkflowEdit = { document: Document; name: string; scope: string };

function activity(document: Document, name: string): GraphNode {
  assertDocument(document);
  const node = findNode(document.graph.root, name);
  if (!node) throw new Error('This activity no longer exists.');
  return node;
}

function enclosing(document: Document, name: string) {
  const parent = pathTo(document.graph.root, name).at(-2);
  return { parent, state: clone(parent ? parent.state : document.graph.initialInput) };
}

function exposedPaths(node: GraphNode): any[] {
  if (isGroup(node)) {
    if (node.promotedStatePaths !== undefined && !Array.isArray(node.promotedStatePaths))
      throw new Error('This activity has unsupported promotions. Repair them before wrapping it.');
    return clone(node.promotedStatePaths ?? []);
  }
  if (!executable(node)) return [];
  if (node.writeBindings !== undefined && !Array.isArray(node.writeBindings))
    throw new Error('This activity has unsupported writes. Repair them before wrapping it.');
  const paths = (node.writeBindings ?? []).map((binding: any) => binding?.target);
  if (
    paths.some(
      (path: any) =>
        !Array.isArray(path) || !path.length || !path.every((part: any) => typeof part === 'string')
    )
  )
    throw new Error('This activity has an invalid write target. Repair it before wrapping it.');
  return [...new Map(paths.map((path: any) => [JSON.stringify(path), clone(path)])).values()];
}

function installWrapper(document: Document, name: string, wrapper: GraphNode): Document {
  const next = clone(document);
  const current = findNode(next.graph.root, name)!;
  // This is structural containment, not a rename. Keep every existing reference and
  // runtime identity attached to the activity now inside the wrapper.
  for (const key of Object.keys(current)) delete current[key];
  Object.assign(current, wrapper);
  assertDocument(next);
  return next;
}

function result(document: Document, name: string, scope: string): WorkflowEdit {
  assertDocument(document);
  return { document, name, scope };
}

function standardContinuation(document: Document, node: GraphNode): GraphNode | undefined {
  if (node.kind !== 'choice' || !node.otherwise || !node.branches.length) return;
  const policies = analyzeWorkflowOutcomes(document).policies.filter(
    (policy) => policy.choiceName === node.name
  );
  return policies.length === node.branches.length ? node.otherwise : undefined;
}

function prependActivity(document: Document, target: GraphNode, kind: string): WorkflowEdit {
  let prepared = document;
  let scope = target.name;
  if (target.kind !== 'seq') {
    scope = uniqueName(document.graph.root, `${target.name}_continuation`);
    prepared = installWrapper(document, target.name, {
      kind: 'seq',
      name: scope,
      state: enclosing(document, target.name).state,
      children: [clone(target)],
      promotedStatePaths: exposedPaths(target),
    });
  }
  const next = addNode(prepared, scope, kind);
  const sequence = findNode(next.document.graph.root, scope)!;
  const added = sequence.children.pop();
  sequence.children.unshift(added);
  return result(next.document, next.name, scope);
}

export function addNext(document: Document, name: string, kind: string): WorkflowEdit {
  const node = activity(document, name);
  if (node.kind === 'succeed' || node.kind === 'fail')
    throw new Error(
      'This terminal ends the run. Add an activity before it or choose another branch.'
    );
  const { parent, state } = enclosing(document, name);
  if (parent?.kind === 'seq') {
    const following =
      parent.children[parent.children.findIndex((child: GraphNode) => child.name === name) + 1];
    const continuation = following && standardContinuation(document, following);
    if (continuation) return prependActivity(document, continuation, kind);
    const next = addNode(document, parent.name, kind, name);
    return result(next.document, next.name, parent.name);
  }
  const scope = uniqueName(document.graph.root, `${name}_sequence`);
  const wrapper: GraphNode = {
    kind: 'seq',
    name: scope,
    state,
    children: [clone(node)],
    promotedStatePaths: exposedPaths(node),
  };
  const wrapped = installWrapper(document, name, wrapper);
  const next = addNode(wrapped, scope, kind, name);
  return result(next.document, next.name, scope);
}

export function addOutcome(document: Document, choiceName: string, kind: string): WorkflowEdit {
  const choice = activity(document, choiceName);
  if (choice.kind !== 'choice') throw new Error('Choose a decision before adding an outcome.');
  const next = addNode(document, choiceName, kind);
  return result(next.document, next.name, choiceName);
}

// No guard truth is inferred. Recognize authored terminal-only paths so an append
// cannot create a disconnected continuation beyond a run-ending activity.
function terminalEndpoint(node: GraphNode): string | undefined {
  if (node.kind === 'succeed' || node.kind === 'fail') return node.name;
  const nested = children(node);
  if (node.kind === 'seq') {
    for (const child of nested) {
      const terminal = terminalEndpoint(child);
      if (terminal) return terminal;
    }
  }
  if (node.kind === 'loop') return terminalEndpoint(node.body);
  if (node.kind === 'choice' && nested.length && nested.every(terminalEndpoint)) return node.name;
  // Parallel consumes branch terminals and returns its group result even when the
  // join cannot be reached. Following checkpoints must remain authorable.
  // An empty map or an exceeded item limit bypasses even a terminal-only body.
  return undefined;
}

export function appendActivity(document: Document, owner: string, kind: string): WorkflowEdit {
  const group = activity(document, owner);
  if (!isGroup(group)) throw new Error('Choose a sequence, decision, or activity region.');
  if (group.kind === 'seq') {
    const last = group.children.at(-1);
    if (
      last?.kind === 'succeed' &&
      analyzeWorkflowOutcomes(document).completions.some(
        (completion) => completion.name === last.name && completion.mode === 'final'
      )
    ) {
      const next = addNode(document, group.name, kind, group.children.at(-2)?.name);
      if (group.children.length === 1) {
        const sequence = findNode(next.document.graph.root, group.name)!;
        sequence.children.unshift(sequence.children.pop());
      }
      return result(next.document, next.name, next.parent);
    }
    const continuation = last && standardContinuation(document, last);
    if (continuation?.kind === 'seq') return appendActivity(document, continuation.name, kind);
    if (
      continuation?.kind === 'succeed' &&
      analyzeWorkflowOutcomes(document).completions.some(
        (completion) => completion.name === continuation.name && completion.mode === 'final'
      )
    )
      return prependActivity(document, continuation, kind);
  }

  if (group.kind === 'seq' || group.kind === 'loop' || group.kind === 'map') {
    const destination = group.kind === 'seq' ? group : group.body;
    const terminal = terminalEndpoint(destination);
    if (terminal)
      throw new Error(
        `${terminal} ends this execution path. Choose an activity before it or add to another outcome.`
      );
  }
  const next = addNode(document, owner, kind);
  return result(next.document, next.name, next.parent);
}

export function wrapActivity(document: Document, name: string, kind: 'loop' | 'par'): WorkflowEdit {
  const node = activity(document, name);
  if (node.kind === 'succeed' || node.kind === 'fail')
    throw new Error('Choose an activity to repeat or run in parallel; a terminal ends the run.');
  if (kind !== 'loop' && kind !== 'par') throw new Error('Choose a loop or parallel group.');
  const { parent, state } = enclosing(document, name);
  const wrapperName = uniqueName(
    document.graph.root,
    `${name}_${kind === 'par' ? 'parallel' : 'loop'}`
  );
  const wrapper: GraphNode = {
    kind,
    name: wrapperName,
    state,
    promotedStatePaths: exposedPaths(node),
    ...(kind === 'loop'
      ? { body: clone(node), maxIterations: 3 }
      : { branches: [clone(node)], join: { kind: 'all' } }),
  };
  return result(installWrapper(document, name, wrapper), wrapperName, parent?.name ?? wrapperName);
}

const validPath = (value: unknown): value is string[] =>
  Array.isArray(value) && value.length > 0 && value.every((part) => typeof part === 'string');
const overlaps = (left: string[], right: string[]) =>
  left.slice(0, Math.min(left.length, right.length)).every((part, index) => part === right[index]);

function independentActivities(document: Document, nodes: GraphNode[]): void {
  const names = new Set(nodes.map((node) => node.name));
  const writes = nodes.map((node) => {
    if (!editableActivityRole(node, bindingFor(document.runtime, node.name)))
      throw new Error('Choose adjacent Agent-backed activities in the same sequence.');
    return exposedPaths(node) as string[][];
  });
  const reads = nodes.map((node) => {
    if (!Array.isArray(node.inputBindings ?? []))
      throw new Error(`Repair ${node.name}’s input mappings before running it in parallel.`);
    const paths: string[][] = [];
    for (const binding of node.inputBindings ?? []) {
      if (!['state', 'item'].includes(binding?.value?.source) || !validPath(binding?.value?.path))
        throw new Error(`Repair ${node.name}’s input mappings before running it in parallel.`);
      if (binding.value.source === 'state') paths.push(binding.value.path);
    }
    const pending: any[] = [node];
    while (pending.length) {
      const value = pending.pop();
      if (!value || typeof value !== 'object') continue;
      const source =
        typeof value.channel === 'string'
          ? value.node
          : ['error', 'signal', 'group'].includes(value.source)
            ? value.name
            : undefined;
      if (source !== node.name && names.has(source))
        throw new Error(`${node.name} references ${source}; keep these activities in sequence.`);
      pending.push(...Object.values(value));
    }
    return paths;
  });
  for (let left = 0; left < nodes.length; left++) {
    for (let right = left + 1; right < nodes.length; right++) {
      if (writes[left].some((path) => writes[right].some((other) => overlaps(path, other))))
        throw new Error(
          `${nodes[left].name} and ${nodes[right].name} write overlapping fields; keep them in sequence.`
        );
      if (writes[left].some((path) => reads[right].some((read) => overlaps(path, read))))
        throw new Error(
          `${nodes[right].name} uses a result from ${nodes[left].name}; keep them in sequence.`
        );
    }
  }
}

/** Existing sibling identities move together; no checkpoint or other scope is crossed. */
export function wrapParallelActivities(
  document: Document,
  name: string,
  additional: string[] = []
): WorkflowEdit {
  if (!additional.length) return wrapActivity(document, name, 'par');
  activity(document, name);
  const { parent, state } = enclosing(document, name);
  if (parent?.kind !== 'seq') throw new Error('Choose adjacent activities in the same sequence.');
  const names = new Set([name, ...additional]);
  const indices = parent.children.flatMap((node: GraphNode, index: number) =>
    names.has(node.name) ? [index] : []
  );
  if (indices.length !== names.size || indices.at(-1)! - indices[0] + 1 !== indices.length)
    throw new Error('Choose one contiguous group of activities in the same sequence.');
  const nodes: GraphNode[] = parent.children.slice(indices[0], indices.at(-1)! + 1);
  independentActivities(document, nodes);
  const wrapperName = uniqueName(document.graph.root, `${name}_parallel`);
  const wrapper: GraphNode = {
    kind: 'par',
    name: wrapperName,
    state,
    branches: clone(nodes),
    join: { kind: 'all' },
    promotedStatePaths: [
      ...new Map(nodes.flatMap(exposedPaths).map((path) => [JSON.stringify(path), path])).values(),
    ],
  };
  const next = clone(document);
  findNode(next.graph.root, parent.name)!.children.splice(indices[0], nodes.length, wrapper);
  return result(next, wrapperName, parent.name);
}

/** The visible choices are safe contiguous intervals around the selected activity. */
export function parallelSiblingCandidates(document: Document, name: string): GraphNode[] {
  const node = findNode(document.graph.root, name);
  const parent = pathTo(document.graph.root, name).at(-2);
  if (
    !node ||
    parent?.kind !== 'seq' ||
    !editableActivityRole(node, bindingFor(document.runtime, name))
  )
    return [];
  const siblings: GraphNode[] = parent.children;
  const at = siblings.findIndex((child) => child.name === name);
  let first = at,
    last = at;
  for (let index = at - 1; index >= 0; index--) {
    try {
      independentActivities(document, siblings.slice(index, at + 1));
      first = index;
    } catch {
      break;
    }
  }
  for (let index = at + 1; index < siblings.length; index++) {
    try {
      independentActivities(document, siblings.slice(at, index + 1));
      last = index;
    } catch {
      break;
    }
  }
  return siblings.slice(first, last + 1);
}

type ProtectionTerm = { conditions: unknown[]; errors: Set<string> };
const errorAtom = (name: string, channel = 'error') => JSON.stringify([name, channel]);
const canonical = (value: unknown): string =>
  JSON.stringify(value, (_, entry) =>
    entry && typeof entry === 'object' && !Array.isArray(entry)
      ? Object.fromEntries(Object.entries(entry).sort(([a], [b]) => a.localeCompare(b)))
      : entry
  );

function guardSources(guard: any): string[] {
  if (!guard || typeof guard !== 'object') return [];
  if (['in', 'k_of_map'].includes(guard.kind)) return [guard.value?.name];
  if (guard.kind === 'k_of_n') return (guard.values ?? []).map((value: any) => value.name);
  if (guard.kind === 'not') return guardSources(guard.guard);
  return (guard.guards ?? []).flatMap(guardSources);
}

function referencesOutcomes(node: GraphNode, watched: Set<string>): boolean {
  const guards =
    node.kind === 'choice'
      ? node.branches.map((branch: any) => branch.when)
      : node.kind === 'loop'
        ? [node.until]
        : node.kind === 'par' && node.join?.kind === 'first'
          ? [node.join.when]
          : [];
  return (
    guards.some((guard: any) => guardSources(guard).some((name) => watched.has(name))) ||
    children(node).some((child) => referencesOutcomes(child, watched))
  );
}

// Read-only compatibility with outcomes.rs::refresh_standard_guard. Rust still
// constructs the policy; this only avoids requesting a rewrite it must reject.
function standardErrorAtoms(guard: any): Set<string> | undefined {
  if (guard?.kind === 'any' && Array.isArray(guard.guards) && guard.guards.length) {
    const parts = guard.guards.map(standardErrorAtoms);
    if (parts.every((part: Set<string> | undefined) => part))
      return new Set(parts.flatMap((part: Set<string>) => [...part]));
  }
  if (
    !['in', 'k_of_map'].includes(guard?.kind) ||
    !Array.isArray(guard.labels) ||
    typeof guard.value?.name !== 'string'
  )
    return;
  const { value, labels } = guard;
  if (
    value.source === 'error' &&
    value.field == null &&
    (guard.kind !== 'k_of_map' || guard.count === 1) &&
    canonical([...new Set(labels)].sort()) ===
      canonical(['crash', 'malformed', 'refusal', 'timeout'])
  )
    return new Set([errorAtom(value.name)]);
  if (
    guard.kind === 'in' &&
    value.source === 'group' &&
    value.field === 'overflow' &&
    labels.length === 1 &&
    labels[0] === 'overflow'
  )
    return new Set([errorAtom(value.name, 'overflow')]);
  return undefined;
}

function policyTerms(guard: any): ProtectionTerm[] | undefined {
  const errors = standardErrorAtoms(guard);
  if (errors) return [{ conditions: [], errors }];
  if (guard?.kind === 'any' && Array.isArray(guard.guards) && guard.guards.length) {
    const terms = guard.guards.map(policyTerms);
    if (terms.every((part: ProtectionTerm[] | undefined) => part)) return terms.flat();
  }
  if (guard?.kind === 'all' && Array.isArray(guard.guards) && guard.guards.length) {
    const errors = standardErrorAtoms(guard.guards.at(-1));
    if (errors) return [{ conditions: guard.guards.slice(0, -1), errors }];
  }
  return undefined;
}

function continuationAllowsProtection(node: GraphNode, suffix: GraphNode[]): boolean {
  const workerNames = (node: GraphNode): string[] =>
    executable(node) ? [node.name] : children(node).flatMap(workerNames);
  const continues = (node: GraphNode): boolean =>
    !['fail', 'succeed'].includes(node.kind) &&
    (node.kind === 'seq' ? children(node).every(continues) : true);
  const expected: ProtectionTerm[] = [];
  const watched = new Set<string>();
  if (node.kind === 'choice') {
    const prior: unknown[] = [];
    for (const branch of node.branches) {
      if (continues(branch.node))
        expected.push({
          conditions: [...prior, branch.when],
          errors: new Set(workerNames(branch.node).map((name) => errorAtom(name))),
        });
      prior.push({ kind: 'not', guard: branch.when });
    }
    if (node.otherwise && continues(node.otherwise))
      expected.push({
        conditions: prior,
        errors: new Set(workerNames(node.otherwise).map((name) => errorAtom(name))),
      });
  } else {
    const errors = new Set(workerNames(node).map((name) => errorAtom(name)));
    if (node.kind === 'map') errors.add(errorAtom(node.name, 'overflow'));
    expected.push({ conditions: [], errors });
  }
  expected.forEach((term) => {
    term.errors.forEach((atom) => watched.add(JSON.parse(atom)[0]));
    term.conditions.flatMap(guardSources).forEach((name) => watched.add(name));
  });
  const first = suffix[0];
  if (!first) return false;
  if (first.kind === 'choice') {
    // A mixed or nonterminal decision belongs to its author, even if one branch
    // is displayed as a folded failure policy. Native refresh requires this exact shape.
    if (
      suffix.length !== 1 ||
      first.branches.length !== 1 ||
      first.branches[0].node.kind !== 'fail' ||
      !first.otherwise
    )
      return false;
    const existing = policyTerms(first.branches[0].when);
    const compatible = existing?.every((old) =>
      expected.some(
        (next) =>
          old.conditions.length <= next.conditions.length &&
          old.conditions.every(
            (condition, index) => canonical(condition) === canonical(next.conditions[index])
          ) &&
          [...old.errors].every((atom) => next.errors.has(atom))
      )
    );
    if (compatible && !referencesOutcomes(first.otherwise, watched)) return true;
  }
  return !suffix.some((child) => referencesOutcomes(child, watched));
}

/** Protect a new activity without replacing an authored recovery decision. */
export function automaticProtectionTarget(document: Document, name: string): string | undefined {
  const path = pathTo(document.graph.root, name);
  const policies = analyzeWorkflowOutcomes(document).policies;
  const simpleContents = (node: GraphNode): boolean =>
    executable(node) ||
    ((node.kind === 'seq' || (node.kind === 'par' && node.join?.kind === 'all')) &&
      children(node).length > 0 &&
      children(node).every(simpleContents));
  const continues = (node: GraphNode): boolean =>
    !['fail', 'succeed'].includes(node.kind) &&
    (node.kind === 'seq' ? children(node).every(continues) : true);
  const supported = (node: GraphNode): boolean => {
    if (['map', 'loop'].includes(node.kind)) return simpleContents(node.body);
    if (node.kind === 'choice') {
      const branches = children(node).filter(continues);
      return branches.length > 0 && branches.every(simpleContents);
    }
    return simpleContents(node);
  };
  const canProtect = (node: GraphNode): boolean => {
    if (!supported(node)) return false;
    const parent = pathTo(document.graph.root, node.name).at(-2);
    if (parent?.kind !== 'seq') return false;
    const index = parent.children.findIndex((child: GraphNode) => child.name === node.name);
    return continuationAllowsProtection(node, parent.children.slice(index + 1));
  };
  const scope =
    path.find(
      (node) =>
        ['par', 'map', 'loop'].includes(node.kind) ||
        (node.kind === 'choice' &&
          node.branches.some(
            (_: unknown, index: number) =>
              !policies.some(
                (policy) => policy.choiceName === node.name && policy.branchIndex === index
              )
          ))
    ) ?? path.at(-1);
  if (scope && canProtect(scope)) return scope.name;

  // A business decision can make a whole repeat unsuitable for automatic
  // protection. A fresh sequential activity still gets its own error checkpoint.
  // Parallel/map branch terminals have group semantics: do not put that fallback
  // inside them, where it would not represent ordinary sequential failure.
  const node = path.at(-1);
  if (
    node &&
    executable(node) &&
    !path.slice(0, -1).some((ancestor) => ['par', 'map'].includes(ancestor.kind)) &&
    canProtect(node)
  )
    return node.name;
  return undefined;
}
