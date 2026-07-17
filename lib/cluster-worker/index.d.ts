export type ArtifactRef = {
  artifactId: string;
  sha256: string;
  byteLength: number;
  mediaType: string;
  typeId: string;
  producer: { node: string; worker: string };
  lineage: { generation: number; runId: string; attempt: number };
  redaction: 'public' | 'internal' | 'confidential' | 'restricted';
};

type RequestBase = {
  isolationProfile: string;
  providerProfile: string;
};

export type LegacyShipRequest = RequestBase &
  (
    | { source: 'issue'; issue: string; prompt?: null; artifacts: [] }
    | { source: 'prompt'; prompt: string; issue?: null; artifacts: [] }
    | { source: 'artifact'; issue?: null; prompt?: null; artifacts: ArtifactRef[] }
  );

export type LegacyShipResult = {
  summary: string;
  status: 'succeeded' | 'failed';
  artifacts: ArtifactRef[];
};

export type LifecycleState =
  | 'idle'
  | 'starting'
  | 'running'
  | 'stopping'
  | 'completed'
  | 'failed'
  | 'timed_out'
  | 'stopped'
  | 'malformed';

export type LifecycleStatus = {
  state: LifecycleState;
  clusterId: string | null;
  sequence: number;
  stopRequested: boolean;
  terminal: boolean;
};

export type WorkerError =
  | {
      status: 'error';
      code: 'timeout' | 'crash' | 'malformed' | 'refusal';
      reason: 'declared_failure';
    }
  | {
      status: 'error';
      code: 'refusal';
      reason: 'policy_denied' | 'interactive_input_required' | 'authentication_required';
    }
  | { status: 'error'; code: 'malformed'; reason: 'malformed_result' };

export type TerminalReceipt =
  | { state: 'completed'; clusterId: string; finishedAt: number; result: LegacyShipResult }
  | {
      state: 'stopped';
      clusterId: string;
      finishedAt: number;
      stop: { requested: true; effective: boolean; externalEffectsRolledBack: false };
    }
  | {
      state: 'failed' | 'timed_out' | 'malformed';
      clusterId: string;
      finishedAt: number;
      outcome: WorkerError;
    };

export type LifecycleEvent = {
  sequence: number;
  state: Exclude<LifecycleState, 'idle'>;
  at: number;
  details?: unknown;
};

export type RunPlan = Readonly<{
  isolation: 'worktree' | 'docker';
  delivery: 'none' | 'pr' | 'ship';
  autoMerge: boolean;
}>;

export type ExecutionBounds = Readonly<{
  executionMs: number;
  shutdownMs: number;
  frameBytes: number;
}>;

export type ResolvedDeploymentProfile = Readonly<{
  isolationProfile: string;
  providerProfile: string;
  plan: RunPlan;
  deployment: Readonly<Record<string, unknown>>;
  provider: Readonly<Record<string, unknown>>;
  bounds: ExecutionBounds;
}>;

export interface DeploymentProfileRegistry {
  readonly bounds?: ExecutionBounds;
  resolve(
    isolationProfile: string,
    providerProfile: string
  ): ResolvedDeploymentProfile | Promise<ResolvedDeploymentProfile>;
}

export interface ArtifactResolver {
  stage(
    artifacts: readonly ArtifactRef[],
    context: Readonly<{ clusterId: string; profile: ResolvedDeploymentProfile }>
  ): Readonly<Record<string, unknown>> | Promise<Readonly<Record<string, unknown>>>;
}

export interface ArtifactReceiptSink {
  collect(
    declared: readonly unknown[],
    context: Readonly<{ clusterId: string; profile: ResolvedDeploymentProfile }>
  ): readonly ArtifactRef[] | Promise<readonly ArtifactRef[]>;
}

export type EngineEvent =
  | { type: 'running' }
  | ({ type: 'complete'; artifacts?: readonly unknown[] } & (
      | { summary: string; result?: never }
      | { result: LegacyShipResult; summary?: string }
    ))
  | ({ type: 'failed' } & Omit<WorkerError, 'status'>)
  | { type: 'malformed' };

export interface EngineAdapter {
  start(input: {
    request: LegacyShipRequest;
    profile: ResolvedDeploymentProfile;
    artifactManifest: Readonly<Record<string, unknown>>;
    clusterId: string;
    onEvent(event: EngineEvent): void;
  }): { clusterId?: string } | Promise<{ clusterId?: string }>;
  status?(): Readonly<Record<string, unknown>> | null;
  stop(): { effective?: boolean } | Promise<{ effective?: boolean }>;
}

export type LegacyClusterWorkerDependencies = {
  profileRegistry?: DeploymentProfileRegistry;
  artifactResolver?: ArtifactResolver | ArtifactResolver['stage'];
  artifactReceiptSink?: ArtifactReceiptSink | ArtifactReceiptSink['collect'];
  engineAdapter?: EngineAdapter;
  clock?: () => number;
  timers?: {
    setTimeout(callback: () => void, milliseconds: number): unknown;
    clearTimeout(handle: unknown): void;
  };
  idFactory?: () => string;
};

export interface LegacyClusterWorker {
  start(request: LegacyShipRequest): Promise<LifecycleStatus>;
  status(): LifecycleStatus;
  events(): AsyncIterableIterator<LifecycleEvent>;
  stop(): Promise<TerminalReceipt>;
  result(): Promise<TerminalReceipt>;
}

export function createLegacyClusterWorker(
  dependencies?: LegacyClusterWorkerDependencies
): LegacyClusterWorker;

export function createDeploymentProfileRegistry(options?: {
  isolationProfiles?: Readonly<Record<string, Readonly<Record<string, unknown>>>>;
  providerProfiles?: Readonly<Record<string, Readonly<Record<string, unknown>>>>;
  bounds?: Partial<ExecutionBounds>;
}): DeploymentProfileRegistry;

export function createCurrentEngineAdapter(options?: object): EngineAdapter;
