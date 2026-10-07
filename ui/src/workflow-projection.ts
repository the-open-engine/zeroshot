import { mapSourceLabel } from './node-data';
import type { Document, GraphNode } from './domain';
import { isObject, isStringList } from './guards';
import { analyzeWorkflowOutcomes } from './workflow-outcomes';

export type WorkflowRole =
  | 'activity'
  | 'decision'
  | 'fork'
  | 'join'
  | 'loop-start'
  | 'loop-end'
  | 'map-start'
  | 'map-end'
  | 'empty'
  | 'collapsed'
  | 'completion';
export type WorkflowNode = {
  id: string;
  owner: string;
  role: WorkflowRole;
  label: string;
  detail?: string;
  kind: string;
};
export type WorkflowEdge = {
  id: string;
  source: string;
  target: string;
  label?: string;
  /** Presentation caption derived from the intact guard, before label text is bounded. */
  shortLabel?: string;
  owner?: string;
  branchIndex?: number;
  otherwise?: boolean;
  repeat?: boolean;
  secondary?: boolean;
};
export type WorkflowRegion = {
  id: string;
  owner: string;
  kind: 'loop' | 'map' | 'par';
  label: string;
  nodeIds: string[];
};
export type WorkflowProjection = {
  nodes: WorkflowNode[];
  edges: WorkflowEdge[];
  regions: WorkflowRegion[];
};

type Exit = { source: string; label?: string; owner?: string };
type Fragment = { entry: string; exits: Exit[]; ids: string[]; attached?: boolean };
type Preceding = { node: GraphNode; fragment: Fragment };
type EdgeDetail = Omit<WorkflowEdge, 'id' | 'source' | 'target'>;

const groups = new Set(['seq', 'choice', 'par', 'loop', 'map']);
const nodeLike = (value: unknown): value is GraphNode =>
  isObject(value) && typeof value.kind === 'string' && typeof value.name === 'string';
const list = (value: unknown): any[] => (Array.isArray(value) ? value : []);
const positive = (value: unknown): value is number =>
  typeof value === 'number' && Number.isSafeInteger(value) && value > 0;
const title = (name: string) => {
  const words = name.replace(/[_-]+/g, ' ');
  return words ? words[0].toUpperCase() + words.slice(1) : 'Unnamed activity';
};
const bounded = (text: string, length = 180) =>
  text.length > length ? `${text.slice(0, length - 1)}…` : text;
// JSON escapes lone surrogates before URI encoding, without changing authored names.
const encodedName = (name: string) => encodeURIComponent(JSON.stringify(name));

function controlLabel(value: any): string {
  if (!isObject(value) || typeof value.name !== 'string') return 'Unspecified result';
  const name = title(value.name);
  if (value.source === 'error') return `${name} error`;
  if ((value.source === 'signal' || value.source === 'group') && typeof value.field === 'string')
    return `${name} · ${value.field}`;
  return `${name} result`;
}

/** Display a guard without evaluating its truth, reachability, or availability. */
export function guardLabel(guard: unknown): string {
  function describe(value: unknown, depth: number): string {
    if (!isObject(value)) return 'Choose a condition';
    if (depth > 5) return 'Nested condition';
    const labels =
      isStringList(value.labels) && value.labels.length
        ? value.labels.slice(0, 4).join(' | ') +
          (value.labels.length > 4 ? ` | +${value.labels.length - 4}` : '')
        : 'choose labels';
    switch (value.kind) {
      case 'in':
        return `${controlLabel(value.value)} is ${labels}`;
      case 'not':
        return `NOT (${describe(value.guard, depth + 1)})`;
      case 'all':
      case 'any': {
        const conditions = list(value.guards);
        if (!conditions.length) return 'Choose conditions';
        return bounded(
          conditions
            .slice(0, 4)
            .map((child) => `(${describe(child, depth + 1)})`)
            .join(value.kind === 'all' ? ' AND ' : ' OR ') + (conditions.length > 4 ? ' …' : '')
        );
      }
      case 'k_of_n':
        return `At least ${positive(value.count) ? value.count : '?'} of ${list(value.values).slice(0, 3).map(controlLabel).join(', ') || 'selected results'} match ${labels}`;
      case 'k_of_map':
        return `At least ${positive(value.count) ? value.count : '?'} items: ${controlLabel(value.value)} is ${labels}`;
      default:
        return 'Advanced condition';
    }
  }
  return bounded(describe(guard, 0));
}

const workerErrors = new Set(['timeout', 'crash', 'malformed', 'refusal']);
const onlyKeys = (value: Record<string, unknown>, keys: string[]) =>
  Object.keys(value).every((key) => keys.includes(key));

/** Recognize common flat guards without parsing display text or evaluating their truth. */
export function compactGuardLabel(guard: unknown): string | undefined {
  function selector(leaf: unknown): Record<string, any> | undefined {
    if (
      !isObject(leaf) ||
      leaf.kind !== 'in' ||
      !onlyKeys(leaf, ['kind', 'value', 'labels']) ||
      !isObject(leaf.value) ||
      !onlyKeys(leaf.value, ['name', 'source', 'field']) ||
      typeof leaf.value.name !== 'string' ||
      !leaf.value.name ||
      !isStringList(leaf.labels)
    )
      return undefined;
    return leaf.value;
  }
  function errorSource(leaf: any): string | undefined {
    const value = selector(leaf);
    return value?.source === 'error' &&
      value.field == null &&
      leaf.labels.length === workerErrors.size &&
      new Set(leaf.labels).size === workerErrors.size &&
      leaf.labels.every((label: string) => workerErrors.has(label))
      ? value.name
      : undefined;
  }
  const single = errorSource(guard);
  if (single) return bounded(`${title(single)} has an error`);
  if (
    !isObject(guard) ||
    !onlyKeys(guard, ['kind', 'guards']) ||
    !Array.isArray(guard.guards) ||
    guard.guards.length < 2
  )
    return undefined;
  if (guard.kind === 'any' && guard.guards.every((leaf) => errorSource(leaf) !== undefined))
    return 'Any error';
  if (
    guard.kind === 'all' &&
    guard.guards.every((leaf) => {
      const value = selector(leaf);
      return (
        value?.source === 'signal' &&
        value.field === 'verdict' &&
        leaf.labels.length === 1 &&
        leaf.labels[0] === 'accepted'
      );
    })
  )
    return 'All accepted';
  return undefined;
}

function childrenOf(node: GraphNode): GraphNode[] {
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

function parallelCanReachJoin(node: GraphNode, continuations: boolean[]): boolean {
  if (!continuations.length) return true; // An editable empty-group placeholder.
  const possible = continuations.filter(Boolean).length;
  if (node.join?.kind === 'all') return possible === continuations.length;
  if (node.join?.kind === 'quorum' && positive(node.join.count)) return possible >= node.join.count;
  return possible > 0;
}

// This deliberately ignores guard truth and provider outcomes. It only prevents
// obvious terminal-only subtrees from acquiring invented continuation edges.
function structurallyContinues(node: GraphNode, ancestors = new Set<GraphNode>()): boolean {
  if (ancestors.has(node) || ancestors.size > 64) return false;
  const next = new Set(ancestors).add(node);
  if (node.kind === 'succeed' || node.kind === 'fail') return false;
  if (node.kind === 'step' || node.kind === 'verifier') return true;
  const children = childrenOf(node);
  if (node.kind === 'seq') return children.every((child) => structurallyContinues(child, next));
  if (node.kind === 'choice')
    return !children.length || children.some((child) => structurallyContinues(child, next));
  // Parallel consumes terminal branches and reports an unmet join as a group
  // result. A terminal branch never becomes a run terminal by presentation.
  if (node.kind === 'par') return true;
  if (node.kind === 'loop') return !children.length || structurallyContinues(children[0], next);
  // Empty input and item-limit overflow bypass even a terminal-only map body.
  if (node.kind === 'map') return true;
  return false;
}

function joinLabel(node: GraphNode): string {
  switch (node.join?.kind) {
    case 'all':
      return 'All branches complete';
    case 'any':
      return 'Any one branch completes';
    case 'quorum':
      return `${positive(node.join.count) ? node.join.count : '?'} branches complete`;
    case 'first':
      return `First matching branch: ${guardLabel(node.join.when)}`;
    default:
      return 'Choose a join rule';
  }
}

function loopLabels(node: GraphNode) {
  const limit = positive(node.maxIterations) ? node.maxIterations : undefined;
  const bounds = limit ? `Up to ${limit} ${limit === 1 ? 'round' : 'rounds'}` : 'Set a round limit';
  const until = node.until == null ? undefined : guardLabel(node.until);
  const continuation = until
    ? `Until met or ${limit ?? 'maximum'} rounds complete`
    : `After ${limit ?? 'maximum'} ${limit === 1 ? 'round' : 'rounds'}`;
  return { limit, bounds, until, continuation };
}

function decisionReadsVerifier(
  choice: GraphNode,
  verifier: GraphNode,
  hidden: Set<number>
): boolean {
  if (
    verifier.kind !== 'verifier' ||
    !isObject(verifier.signals) ||
    !onlyKeys(choice, ['kind', 'name', 'state', 'promotedStatePaths', 'branches', 'otherwise']) ||
    !nodeLike(choice.otherwise)
  )
    return false;
  const visible = list(choice.branches).filter((_, index) => !hidden.has(index));
  return (
    visible.length > 0 &&
    visible.every((branch) => {
      if (!isObject(branch) || !onlyKeys(branch, ['when', 'node']) || !nodeLike(branch.node))
        return false;
      const guard = branch?.when;
      if (
        !isObject(guard) ||
        guard.kind !== 'in' ||
        !onlyKeys(guard, ['kind', 'value', 'labels']) ||
        !isObject(guard.value) ||
        !onlyKeys(guard.value, ['name', 'source', 'field']) ||
        guard.value.source !== 'signal' ||
        guard.value.name !== verifier.name ||
        typeof guard.value.field !== 'string' ||
        !isStringList(guard.labels) ||
        !guard.labels.length
      )
        return false;
      const domain = verifier.signals[guard.value.field];
      return isStringList(domain) && guard.labels.every((label) => domain.includes(label));
    })
  );
}

/** Project the authored tree into workflow structure; never normalize or mutate it. */
export function projectWorkflow(
  document: Document,
  collapsed: Set<string> = new Set()
): WorkflowProjection {
  const outcomes = analyzeWorkflowOutcomes(document);
  const hiddenBranches = new Map<string, Set<number>>();
  for (const policy of outcomes.policies) {
    const branches = hiddenBranches.get(policy.choiceName) ?? new Set<number>();
    branches.add(policy.branchIndex);
    hiddenBranches.set(policy.choiceName, branches);
  }
  const finalCompletions = new Set(
    outcomes.completions
      .filter((completion) => completion.mode === 'final')
      .map((completion) => completion.name)
  );
  const branchTerminals = new Set<string>();
  function collectBranchTerminals(node: GraphNode, ancestors: Set<GraphNode>, inParallel = false) {
    if (ancestors.has(node) || ancestors.size > 64) return;
    if (inParallel && ['succeed', 'fail'].includes(node.kind)) branchTerminals.add(node.name);
    const next = new Set(ancestors).add(node);
    childrenOf(node).forEach((child) =>
      collectBranchTerminals(child, next, inParallel || node.kind === 'par')
    );
  }
  collectBranchTerminals(document.graph.root, new Set());
  return new WorkflowProjectionBuilder(
    document,
    collapsed,
    outcomes,
    hiddenBranches,
    finalCompletions,
    branchTerminals
  ).project();
}

class WorkflowProjectionBuilder {
  private readonly result: WorkflowProjection = { nodes: [], edges: [], regions: [] };
  private readonly used = new Set<string>();

  constructor(
    private readonly document: Document,
    private readonly collapsed: Set<string>,
    private readonly outcomes: ReturnType<typeof analyzeWorkflowOutcomes>,
    private readonly hiddenBranches: Map<string, Set<number>>,
    private readonly finalCompletions: Set<string>,
    private readonly branchTerminals: Set<string>
  ) {}

  project(): WorkflowProjection {
    this.visit(this.document.graph.root, new Set());
    return this.result;
  }

  private unique(preferred: string): string {
    let id = preferred,
      suffix = 2;
    while (this.used.has(id)) id = `${preferred}:${suffix++}`;
    this.used.add(id);
    return id;
  }
  private addNode(owner: GraphNode, role: WorkflowRole, label: string, detail?: string): string {
    const id = this.unique(
      `workflow:${role === 'activity' ? 'node' : 'virtual'}:${encodedName(owner.name)}:${role}`
    );
    this.result.nodes.push({
      id,
      owner: owner.name,
      role,
      label,
      ...(detail ? { detail } : {}),
      kind: owner.kind,
    });
    return id;
  }
  private edge(source: string, target: string, detail: EdgeDetail = {}) {
    this.result.edges.push({
      id: `workflow:edge:${this.result.edges.length + 1}`,
      source,
      target,
      ...detail,
    });
  }
  private connect(exits: Exit[], target: string, owner: GraphNode, detail: EdgeDetail = {}) {
    for (const exit of exits)
      this.edge(exit.source, target, {
        owner: exit.owner ?? owner.name,
        ...(exit.label ? { label: exit.label } : {}),
        ...detail,
      });
  }
  private region(owner: GraphNode, kind: WorkflowRegion['kind'], label: string, ids: string[]) {
    this.result.regions.push({
      id: this.unique(`workflow:region:${encodedName(owner.name)}:${kind}`),
      owner: owner.name,
      kind,
      label,
      nodeIds: [...ids],
    });
  }
  private empty(owner: GraphNode, label: string): Fragment {
    const id = this.addNode(owner, 'empty', label, 'Draft · add workflow content');
    return { entry: id, exits: [{ source: id }], ids: [id] };
  }

  private visit(node: GraphNode, ancestors: Set<GraphNode>, preceding?: Preceding): Fragment {
    if (ancestors.has(node) || ancestors.size > 64)
      return this.empty(node, 'Nesting limit reached');
    const next = new Set(ancestors).add(node);
    // These routes remain authored controls and editable policies at the same
    // checkpoint. Removing their presentation does not move state or bindings.
    if (
      node.kind === 'choice' &&
      list(node.branches).length > 0 &&
      this.hiddenBranches.get(node.name)?.size === list(node.branches).length &&
      nodeLike(node.otherwise)
    )
      return this.visit(node.otherwise, next);
    if (groups.has(node.kind) && this.collapsed.has(node.name)) {
      const loop = node.kind === 'loop' ? loopLabels(node) : undefined;
      const description = loop
        ? `${loop.bounds}${loop.until ? ` · Stop when ${loop.until}` : ''}`
        : node.kind === 'map'
          ? 'For each item'
          : node.kind === 'par'
            ? joinLabel(node)
            : node.kind === 'choice'
              ? 'Ordered decision'
              : 'Subprocess';
      const id = this.addNode(node, 'collapsed', title(node.name), `${description} · collapsed`);
      return {
        entry: id,
        exits: structurallyContinues(node)
          ? [{ source: id, ...(loop ? { owner: node.name, label: loop.continuation } : {}) }]
          : [],
        ids: [id],
      };
    }
    switch (node.kind) {
      case 'seq':
        return this.visitSeq(node, next);
      case 'choice':
        return this.visitChoice(node, next, preceding);
      case 'par':
        return this.visitPar(node, next);
      case 'loop':
        return this.visitLoop(node, next);
      case 'map':
        return this.visitMap(node, next);
      default:
        return this.visitDefault(node);
    }
  }
  private visitSeq(node: GraphNode, next: Set<GraphNode>): Fragment {
    const children = childrenOf(node);
    if (!children.length) return this.empty(node, 'Add first step');
    const fragments: Fragment[] = [];
    children.forEach((child, index) => {
      fragments.push(
        this.visit(
          child,
          next,
          index ? { node: children[index - 1], fragment: fragments[index - 1] } : undefined
        )
      );
    });
    let exits = fragments[0].exits;
    for (const fragment of fragments.slice(1)) {
      if (exits.length) {
        if (!fragment.attached) this.connect(exits, fragment.entry, node);
        exits = fragment.exits;
      }
      // Preserve unreachable authored draft nodes, but do not draw an edge
      // from a terminal or restart flow at the following disconnected node.
    }
    return {
      entry: fragments[0].entry,
      exits,
      ids: fragments.flatMap((fragment) => fragment.ids),
    };
  }

  private visitChoice(node: GraphNode, next: Set<GraphNode>, preceding?: Preceding): Fragment {
    const branches = list(node.branches);
    if (!branches.length && !nodeLike(node.otherwise)) return this.empty(node, 'Add an outcome');
    const attached =
      !!preceding &&
      preceding.fragment.ids.length === 1 &&
      preceding.fragment.exits.length === 1 &&
      preceding.fragment.exits[0].source === preceding.fragment.entry &&
      decisionReadsVerifier(node, preceding.node, this.hiddenBranches.get(node.name) ?? new Set());
    const decision = attached
      ? preceding!.fragment.entry
      : this.addNode(
          node,
          'decision',
          title(node.name),
          'First matching outcome wins · evaluate top to bottom'
        );
    const ids = attached ? [] : [decision],
      exits: Exit[] = [];
    branches.forEach((branch, index) => {
      if (this.hiddenBranches.get(node.name)?.has(index)) return;
      const label = `${index + 1}. ${guardLabel(branch?.when)}`;
      const caption = compactGuardLabel(branch?.when);
      const fragment = nodeLike(branch?.node)
        ? this.visit(branch.node, next)
        : this.empty(node, `Add outcome ${index + 1}`);
      this.edge(decision, fragment.entry, {
        owner: node.name,
        branchIndex: index,
        label,
        shortLabel: caption ? `${index + 1}. ${caption}` : label,
      });
      ids.push(...fragment.ids);
      exits.push(...fragment.exits);
    });
    if (nodeLike(node.otherwise)) {
      const fragment = this.visit(node.otherwise, next);
      this.edge(decision, fragment.entry, {
        owner: node.name,
        otherwise: true,
        label: 'OTHERWISE · no earlier match',
      });
      ids.push(...fragment.ids);
      exits.push(...fragment.exits);
    }
    return { entry: decision, exits, ids, ...(attached ? { attached: true } : {}) };
  }

  private visitPar(node: GraphNode, next: Set<GraphNode>): Fragment {
    const branches = childrenOf(node);
    if (!branches.length) return this.empty(node, 'Add parallel branches');
    const fork = this.addNode(
      node,
      'fork',
      title(node.name),
      `Parallel branches · ${joinLabel(node)}`
    );
    const fragments = branches.map((branch, index) => {
      const fragment = this.visit(branch, next);
      this.edge(fork, fragment.entry, { owner: node.name, branchIndex: index });
      return fragment;
    });
    const ids = [fork, ...fragments.flatMap((fragment) => fragment.ids)];
    const reachableJoin = parallelCanReachJoin(
      node,
      fragments.map((fragment) => fragment.exits.length > 0)
    );
    let exits: Exit[] = [];
    {
      const join = this.addNode(
        node,
        'join',
        reachableJoin ? joinLabel(node) : 'Parallel group ends',
        !reachableJoin
          ? 'Terminal branches cannot satisfy this join; handle its group result.'
          : node.join?.kind === 'first'
            ? 'The rule may finish without a matching branch; handle its group result.'
            : undefined
      );
      for (const fragment of fragments) this.connect(fragment.exits, join, node);
      if (!reachableJoin)
        this.edge(fork, join, {
          owner: node.name,
          secondary: true,
          label: 'Branches settle · join not reached',
        });
      ids.push(join);
      exits = [{ source: join }];
    }
    this.region(node, 'par', `${title(node.name)} · ${joinLabel(node)}`, ids);
    return { entry: fork, exits, ids };
  }

  private visitLoop(node: GraphNode, next: Set<GraphNode>): Fragment {
    const { limit, bounds, until, continuation } = loopLabels(node);
    const exhaustionStops = this.outcomes.policies.some(
      (policy) =>
        policy.sources.includes(node.name) &&
        policy.guard.kind === 'in' &&
        policy.guard.value?.name === node.name &&
        policy.guard.value?.source === 'group' &&
        policy.guard.value?.field === 'terminated' &&
        policy.guard.labels?.length === 1 &&
        policy.guard.labels[0] === 'exhausted'
    );
    const untilParts = node.until?.kind === 'any' ? list(node.until.guards) : [node.until];
    const review = untilParts.find((part) => part?.value?.source === 'signal');
    const approved =
      exhaustionStops &&
      review?.kind === 'in' &&
      review.value?.field === 'verdict' &&
      review.labels?.length === 1 &&
      review.labels[0] === 'accepted' &&
      untilParts.every(
        (part) =>
          part === review ||
          (part?.kind === 'in' &&
            part.value?.source === 'error' &&
            part.value.field == null &&
            part.labels?.length === workerErrors.size &&
            new Set(part.labels).size === workerErrors.size &&
            part.labels.every((label: string) => workerErrors.has(label)) &&
            this.outcomes.policies.some(
              (policy) => policy.owner === node.name && policy.sources.includes(part.value.name)
            ))
      );
    const start = this.addNode(
      node,
      'loop-start',
      title(node.name),
      `${bounds}${until ? ` · Stop when ${until}` : ''}`
    );
    const body = nodeLike(node.body)
      ? this.visit(node.body, next)
      : this.empty(node, 'Add repeated work');
    this.edge(start, body.entry, { owner: node.name, label: 'First round' });
    const ids = [start, ...body.ids];
    let exits: Exit[] = [];
    if (body.exits.length) {
      const end = this.addNode(
        node,
        'loop-end',
        'Round complete',
        until ? `Check ${until}; otherwise respect the round limit.` : bounds
      );
      this.connect(body.exits, end, node);
      ids.push(end);
      if (limit && limit > 1)
        this.edge(end, body.entry, {
          owner: node.name,
          repeat: true,
          label: approved
            ? `Try again · up to ${limit} attempts`
            : `${until ? 'Until not met · ' : ''}More rounds remain (max ${limit})`,
        });
      exits = [
        {
          source: end,
          owner: node.name,
          label: approved ? 'Approved' : continuation,
        },
      ];
    }
    this.region(node, 'loop', `${title(node.name)} · ${bounds}`, ids);
    return { entry: start, exits, ids };
  }

  private visitMap(node: GraphNode, next: Set<GraphNode>): Fragment {
    const limit = positive(node.maxItems) ? node.maxItems : undefined;
    const start = this.addNode(
      node,
      'map-start',
      title(node.name),
      `${mapSourceLabel(this.document, node)}${limit ? ` · Up to ${limit} items` : ''}`
    );
    const body = nodeLike(node.body)
      ? this.visit(node.body, next)
      : this.empty(node, 'Add item work');
    const end = this.addNode(node, 'map-end', 'Results', undefined);
    this.edge(start, body.entry, { owner: node.name, label: 'Each item' });
    this.connect(body.exits, end, node, { label: 'Item completes' });
    this.edge(start, end, {
      owner: node.name,
      secondary: true,
      label: 'No items or item limit exceeded',
    });
    if (body.exits.length && limit && limit > 1)
      this.edge(end, body.entry, {
        owner: node.name,
        repeat: true,
        secondary: true,
        label: 'Repeat independently for other items',
      });
    const ids = [start, ...body.ids, end];
    this.region(node, 'map', `${title(node.name)} · For each item`, ids);
    return {
      entry: start,
      exits: [{ source: end, owner: node.name, label: 'Items processed or limit exceeded' }],
      ids,
    };
  }

  private visitDefault(node: GraphNode): Fragment {
    const known = ['step', 'verifier', 'succeed', 'fail'].includes(node.kind);
    const terminal = node.kind === 'succeed' || node.kind === 'fail';
    const branchTerminal = this.branchTerminals.has(node.name);
    const detail =
      node.kind === 'succeed'
        ? branchTerminal
          ? 'End this branch; the parallel join determines what follows'
          : this.finalCompletions.has(node.name)
            ? 'Return the configured run result'
            : 'Finish the whole run here'
        : node.kind === 'fail'
          ? `${branchTerminal ? 'Stop this branch' : 'Stop run'}${typeof node.reason === 'string' ? ` · ${node.reason}` : ''}`
          : node.kind === 'verifier'
            ? 'Verifier'
            : node.kind === 'step'
              ? 'Agent'
              : 'Advanced node · inspect its contract';
    const label =
      node.kind === 'succeed'
        ? branchTerminal
          ? 'End branch'
          : this.finalCompletions.has(node.name)
            ? 'Complete'
            : 'Finish run early'
        : title(node.name);
    const id = this.addNode(node, terminal ? 'completion' : 'activity', label, detail);
    return {
      entry: id,
      exits: known && (node.kind === 'step' || node.kind === 'verifier') ? [{ source: id }] : [],
      ids: [id],
    };
  }
}
