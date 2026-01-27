# Zeroshot SaaS: Architecture Design Document

> Multi-Tenant Agent Orchestration Platform

**Version:** 1.0.0
**Date:** 2026-01-24
**Status:** Draft

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Technology Stack](#2-technology-stack)
3. [System Architecture](#3-system-architecture)
4. [Core Components](#4-core-components)
5. [Data Architecture](#5-data-architecture)
6. [Temporal Workflows](#6-temporal-workflows)
7. [Agent Execution Runtime](#7-agent-execution-runtime)
8. [API Design](#8-api-design)
9. [Multi-Tenancy](#9-multi-tenancy)
10. [Security Architecture](#10-security-architecture)
11. [Observability](#11-observability)
12. [Deployment Topology](#12-deployment-topology)
13. [Migration Strategy](#13-migration-strategy)
14. [12-Factor Compliance](#14-12-factor-compliance)

---

## 1. Executive Summary

### 1.1 Vision

Transform Zeroshot from a single-user CLI tool into a horizontally-scalable, multi-tenant SaaS platform for autonomous AI agent orchestration.

### 1.2 Key Design Principles

| Principle                    | Implementation                                                             |
| ---------------------------- | -------------------------------------------------------------------------- |
| **API-First**                | All functionality exposed via REST/WebSocket APIs                          |
| **12-Factor**                | Stateless services, config from environment, backing services as resources |
| **Event-Driven**             | Redis Streams for decoupled, async communication                           |
| **Immutable Infrastructure** | Docker + gVisor containers for isolated, reproducible agent execution      |
| **Observable**               | Structured logging, metrics, distributed tracing from day one              |

### 1.3 Technology Choices

| Concern       | Technology            | Rationale                                                                |
| ------------- | --------------------- | ------------------------------------------------------------------------ |
| Orchestration | Temporal.io           | Battle-tested workflow engine, handles retries/timeouts/versioning       |
| Message Bus   | Redis Streams         | Persistent, ordered, consumer groups, simpler than Kafka                 |
| Compute       | HashiCorp Nomad       | Lightweight orchestrator, native Docker support                          |
| Isolation     | Docker + gVisor       | OCI-compatible, strong isolation via user-space kernel, minimal overhead |
| Database      | PostgreSQL            | ACID, JSONB, row-level security for multi-tenancy                        |
| Storage       | S3-compatible (MinIO) | Artifacts, logs, git bundles                                             |
| Cache         | Redis                 | Session, rate limiting, ephemeral state                                  |

---

## 2. Technology Stack

### 2.1 Infrastructure Layer

```
┌─────────────────────────────────────────────────────────────────────┐
│                        Infrastructure Layer                          │
├─────────────────┬─────────────────┬─────────────────┬───────────────┤
│     Nomad       │   Consul        │    Vault        │   Terraform   │
│  (Scheduling)   │ (Discovery)     │  (Secrets)      │   (IaC)       │
├─────────────────┴─────────────────┴─────────────────┴───────────────┤
│                 Docker + gVisor Containers (Agent Runtime)           │
├─────────────────────────────────────────────────────────────────────┤
│                         Linux Hosts (VMs / EC2 / Bare Metal)         │
└─────────────────────────────────────────────────────────────────────┘
```

### 2.2 Application Layer

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Application Layer                            │
├───────────────┬───────────────┬───────────────┬─────────────────────┤
│   API         │   Temporal    │   Stream      │   Webhook           │
│   Gateway     │   Workers     │   Processors  │   Handlers          │
├───────────────┴───────────────┴───────────────┴─────────────────────┤
│                         Service Mesh (Consul Connect)                │
└─────────────────────────────────────────────────────────────────────┘
```

### 2.3 Data Layer

```
┌─────────────────────────────────────────────────────────────────────┐
│                           Data Layer                                 │
├─────────────────┬─────────────────┬─────────────────┬───────────────┤
│   PostgreSQL    │  Redis Streams  │   S3 (MinIO)    │  Redis Cache  │
│   (Ledger)      │  (Events)       │   (Artifacts)   │  (Sessions)   │
└─────────────────┴─────────────────┴─────────────────┴───────────────┘
```

---

## 3. System Architecture

### 3.1 High-Level Architecture

```
                                 ┌─────────────────┐
                                 │   Load Balancer │
                                 │   (Traefik)     │
                                 └────────┬────────┘
                                          │
                    ┌─────────────────────┼─────────────────────┐
                    │                     │                     │
              ┌─────▼─────┐        ┌──────▼──────┐       ┌──────▼──────┐
              │    API    │        │  WebSocket  │       │   Webhook   │
              │  Service  │        │   Gateway   │       │   Ingress   │
              └─────┬─────┘        └──────┬──────┘       └──────┬──────┘
                    │                     │                     │
                    └──────────┬──────────┴──────────┬──────────┘
                               │                     │
                    ┌──────────▼──────────┐   ┌──────▼──────┐
                    │   Temporal Server   │   │ Redis Streams│
                    │   (Workflow Engine) │   │ (Event Bus)  │
                    └──────────┬──────────┘   └──────┬───────┘
                               │                     │
         ┌─────────────────────┼─────────────────────┤
         │                     │                     │
   ┌─────▼─────┐        ┌──────▼──────┐       ┌──────▼──────┐
   │ Cluster   │        │   Agent     │       │   Stream    │
   │ Worker    │        │   Worker    │       │  Processor  │
   │ (Temporal)│        │  (Temporal) │       │  (Consumer) │
   └─────┬─────┘        └──────┬──────┘       └─────────────┘
         │                     │
         │              ┌──────▼──────┐
         │              │   Nomad     │
         │              │  Scheduler  │
         │              └──────┬──────┘
         │                     │
         │              ┌──────▼──────┐
         │              │   Docker    │
         │              │  + gVisor   │
         │              └─────────────┘
         │
   ┌─────▼─────────────────────────────────────────────────┐
   │                    Data Stores                         │
   ├───────────────┬───────────────┬───────────────────────┤
   │  PostgreSQL   │     Redis     │    S3 (MinIO)         │
   │  (Primary)    │   (Cache)     │   (Artifacts)         │
   └───────────────┴───────────────┴───────────────────────┘
```

### 3.2 Request Flow

```
1. Client → API Gateway → Authentication/Rate Limiting
2. API Gateway → API Service → Validate & Persist Cluster
3. API Service → Temporal → Start ClusterWorkflow
4. ClusterWorkflow → Conductor Activity → Classify Task
5. ClusterWorkflow → Spawn Agent Child Workflows
6. AgentWorkflow → Nomad API → Schedule Docker Job (gVisor runtime)
7. Docker Container → Execute Claude Task → Stream Output
8. Output → Redis Streams → WebSocket Gateway → Client
9. AgentWorkflow → Complete → Parent Notified
10. ClusterWorkflow → All Agents Done → Finalize
```

### 3.3 Component Responsibilities

| Component         | Responsibility                          | Scaling Strategy                 |
| ----------------- | --------------------------------------- | -------------------------------- |
| API Service       | REST endpoints, validation, persistence | Horizontal (stateless)           |
| WebSocket Gateway | Real-time streaming to clients          | Horizontal (sticky sessions)     |
| Temporal Server   | Workflow orchestration, durability      | Temporal Cloud or self-hosted HA |
| Cluster Worker    | Cluster lifecycle workflows             | Horizontal (Temporal workers)    |
| Agent Worker      | Agent execution workflows               | Horizontal (Temporal workers)    |
| Stream Processor  | Event fan-out, aggregation              | Consumer groups                  |
| Nomad             | Docker/gVisor job scheduling            | Multi-node cluster               |

---

## 4. Core Components

### 4.1 API Service

Stateless REST API service handling all client interactions.

```
src/
├── api/
│   ├── server.ts              # Express/Fastify app
│   ├── routes/
│   │   ├── clusters.ts        # Cluster CRUD
│   │   ├── agents.ts          # Agent management
│   │   ├── messages.ts        # Ledger queries
│   │   ├── templates.ts       # Template management
│   │   └── webhooks.ts        # GitHub/GitLab webhooks
│   ├── middleware/
│   │   ├── auth.ts            # JWT/API key validation
│   │   ├── rateLimit.ts       # Per-tenant rate limiting
│   │   ├── tenant.ts          # Tenant context injection
│   │   └── validation.ts      # Request validation (Zod)
│   └── services/
│       ├── clusterService.ts  # Business logic
│       ├── temporalClient.ts  # Temporal SDK wrapper
│       └── streamPublisher.ts # Redis Streams publisher
```

**Key Design Decisions:**

1. **Stateless**: All state in PostgreSQL/Redis, horizontal scaling trivial
2. **Request Validation**: Zod schemas for type-safe validation
3. **Tenant Context**: Injected via middleware, propagated to all services
4. **Idempotency**: Client-provided idempotency keys for safe retries

### 4.2 WebSocket Gateway

Real-time event streaming to connected clients.

```typescript
// Connection handling
interface ClientConnection {
  tenantId: string;
  userId: string;
  subscriptions: Set<string>; // cluster IDs
  socket: WebSocket;
}

// Message routing
Redis Streams (cluster:{id}:events)
    → Stream Processor
    → Redis Pub/Sub (fan-out)
    → WebSocket Gateway
    → Client
```

**Scaling Strategy:**

- Sticky sessions via consistent hashing on connection ID
- Redis Pub/Sub for cross-instance message delivery
- Reconnection with cursor-based replay from Redis Streams

### 4.3 Temporal Workers

#### 4.3.1 Cluster Worker

Handles cluster lifecycle as a Temporal workflow.

```typescript
// Workflow definition (see Section 6 for details)
@workflow
class ClusterWorkflow {
  // Durable state
  private state: ClusterState;
  private agents: Map<string, AgentHandle>;

  // Signals (external events)
  @signal
  async stop(): Promise<void>;

  @signal
  async resume(prompt?: string): Promise<void>;

  // Queries (read state)
  @query
  getState(): ClusterState;

  // Main execution
  async execute(input: ClusterInput): Promise<ClusterResult>;
}
```

#### 4.3.2 Agent Worker

Handles individual agent execution.

```typescript
@workflow
class AgentWorkflow {
  async execute(input: AgentInput): Promise<AgentResult> {
    // 1. Build context from ledger
    const context = await buildContext(input);

    // 2. Schedule Docker job via Nomad (gVisor runtime)
    const jobId = await scheduleDockerJob(input, context);

    // 3. Wait for completion (with timeout)
    const result = await waitForJob(jobId, input.timeout);

    // 4. Execute hooks
    await executeHooks(input.hooks, result);

    return result;
  }
}
```

### 4.4 Stream Processor

Consumes Redis Streams for event processing.

```typescript
// Consumer group processing
const GROUP = 'zeroshot-processors';
const CONSUMER = `processor-${hostname()}`;

async function processEvents() {
  while (true) {
    const events = await redis.xreadgroup(
      'GROUP',
      GROUP,
      CONSUMER,
      'STREAMS',
      'cluster:*:events',
      '>'
    );

    for (const event of events) {
      await processEvent(event);
      await redis.xack(stream, GROUP, event.id);
    }
  }
}

async function processEvent(event: StreamEvent) {
  switch (event.type) {
    case 'AGENT_OUTPUT':
      // Fan out to WebSocket subscribers
      await fanOutToSubscribers(event);
      break;
    case 'TOKEN_USAGE':
      // Update billing meters
      await updateUsageMeters(event);
      break;
    case 'CLUSTER_COMPLETE':
      // Trigger webhooks
      await triggerWebhooks(event);
      break;
  }
}
```

### 4.5 Nomad Job Scheduler

Interface between Temporal and Docker/gVisor execution.

```typescript
// Nomad job template for agent execution
const agentJobSpec = {
  ID: `agent-${agentId}`,
  Type: 'batch',
  TaskGroups: [
    {
      Name: 'agent',
      Tasks: [
        {
          Name: 'claude-task',
          Driver: 'docker',
          Config: {
            image: 'zeroshot/agent:latest',
            runtime: 'runsc', // gVisor runtime
            cpu_hard_limit: true,
            memory_hard_limit: true,
            network_mode: 'bridge',
            labels: {
              cluster_id: clusterId,
              agent_id: agentId,
              tenant_id: tenantId,
            },
            volumes: ['local/context.json:/app/context.json:ro', 'local/workspace:/workspace'],
          },
          Templates: [
            {
              // Inject context as file
              DestPath: 'local/context.json',
              Data: contextJson,
            },
          ],
          Env: {
            ANTHROPIC_API_KEY:
              '{{with secret "kv/tenants/{{tenant_id}}/anthropic"}}{{.Data.api_key}}{{end}}',
            REDIS_URL: '{{env "REDIS_URL"}}',
            CLUSTER_ID: clusterId,
            AGENT_ID: agentId,
          },
        },
      ],
    },
  ],
};
```

---

## 5. Data Architecture

### 5.1 PostgreSQL Schema

```sql
-- Tenants
CREATE TABLE tenants (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    slug TEXT UNIQUE NOT NULL,
    plan TEXT NOT NULL DEFAULT 'free',
    settings JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Users (members of tenants)
CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id),
    email TEXT NOT NULL,
    name TEXT,
    role TEXT NOT NULL DEFAULT 'member',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, email)
);

-- API Keys
CREATE TABLE api_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id),
    key_hash TEXT NOT NULL UNIQUE,
    key_prefix TEXT NOT NULL, -- First 8 chars for identification
    name TEXT NOT NULL,
    scopes TEXT[] NOT NULL DEFAULT '{}',
    last_used_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Clusters
CREATE TABLE clusters (
    id TEXT PRIMARY KEY,
    tenant_id UUID NOT NULL REFERENCES tenants(id),

    -- Input
    issue_source TEXT NOT NULL,
    issue_identifier TEXT NOT NULL,
    issue_title TEXT,
    issue_body TEXT,

    -- Configuration
    template_name TEXT,
    template_params JSONB NOT NULL DEFAULT '{}',
    isolation_mode TEXT NOT NULL DEFAULT 'gvisor',

    -- State
    state TEXT NOT NULL DEFAULT 'pending',
    workflow_id TEXT, -- Temporal workflow ID

    -- Metadata
    created_by UUID REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,

    -- Indexes for common queries
    CONSTRAINT valid_state CHECK (state IN (
        'pending', 'initializing', 'running', 'stopping',
        'stopped', 'completed', 'failed', 'killed'
    ))
);

CREATE INDEX idx_clusters_tenant_state ON clusters(tenant_id, state);
CREATE INDEX idx_clusters_tenant_created ON clusters(tenant_id, created_at DESC);

-- Agents (within clusters)
CREATE TABLE agents (
    id TEXT NOT NULL,
    cluster_id TEXT NOT NULL REFERENCES clusters(id) ON DELETE CASCADE,
    tenant_id UUID NOT NULL REFERENCES tenants(id),

    -- Configuration
    role TEXT NOT NULL,
    model_level TEXT NOT NULL,
    config JSONB NOT NULL,

    -- State
    state TEXT NOT NULL DEFAULT 'idle',
    iteration INTEGER NOT NULL DEFAULT 0,

    -- Execution
    nomad_job_id TEXT,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,

    PRIMARY KEY (cluster_id, id)
);

-- Messages (the ledger)
CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    cluster_id TEXT NOT NULL REFERENCES clusters(id) ON DELETE CASCADE,
    tenant_id UUID NOT NULL REFERENCES tenants(id),

    -- Message content
    timestamp BIGINT NOT NULL,
    topic TEXT NOT NULL,
    sender TEXT NOT NULL,
    receiver TEXT NOT NULL DEFAULT 'broadcast',
    content_text TEXT,
    content_data JSONB,
    metadata JSONB,

    -- Ordering
    sequence_num BIGSERIAL
);

CREATE INDEX idx_messages_cluster_topic ON messages(cluster_id, topic);
CREATE INDEX idx_messages_cluster_seq ON messages(cluster_id, sequence_num);
CREATE INDEX idx_messages_tenant ON messages(tenant_id);

-- Token Usage (for billing)
CREATE TABLE token_usage (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id),
    cluster_id TEXT NOT NULL REFERENCES clusters(id),
    agent_id TEXT NOT NULL,

    model TEXT NOT NULL,
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    cost_usd DECIMAL(10, 6) NOT NULL,

    recorded_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_token_usage_tenant_date ON token_usage(tenant_id, recorded_at);

-- Row Level Security
ALTER TABLE clusters ENABLE ROW LEVEL SECURITY;
ALTER TABLE agents ENABLE ROW LEVEL SECURITY;
ALTER TABLE messages ENABLE ROW LEVEL SECURITY;
ALTER TABLE token_usage ENABLE ROW LEVEL SECURITY;

-- RLS Policies (tenant isolation)
CREATE POLICY tenant_isolation_clusters ON clusters
    USING (tenant_id = current_setting('app.tenant_id')::UUID);

CREATE POLICY tenant_isolation_agents ON agents
    USING (tenant_id = current_setting('app.tenant_id')::UUID);

CREATE POLICY tenant_isolation_messages ON messages
    USING (tenant_id = current_setting('app.tenant_id')::UUID);

CREATE POLICY tenant_isolation_usage ON token_usage
    USING (tenant_id = current_setting('app.tenant_id')::UUID);
```

### 5.2 Redis Data Structures

```
# Streams (Event Log)
cluster:{cluster_id}:events          # All events for a cluster
    → {id: "1234-0", type: "AGENT_OUTPUT", data: {...}}

tenant:{tenant_id}:events            # All events for a tenant (aggregated)

# Pub/Sub (Real-time Fan-out)
ws:cluster:{cluster_id}              # WebSocket subscribers for cluster

# Hashes (Fast Lookups)
cluster:{cluster_id}:state           # Current cluster state
    → { state: "running", agents: [...], ... }

agent:{cluster_id}:{agent_id}:state  # Current agent state
    → { state: "executing", iteration: 2, ... }

# Sorted Sets (Rate Limiting)
ratelimit:{tenant_id}:api            # API request timestamps
    → { timestamp: score }

# Strings (Locks & Coordination)
lock:cluster:{cluster_id}            # Distributed lock
    → "worker-1" (with TTL)

# Sets (Active Connections)
ws:connections:{instance_id}         # Connections on this instance
    → { connection_id, ... }
```

### 5.3 S3 Object Structure

```
s3://zeroshot-artifacts/
├── tenants/
│   └── {tenant_id}/
│       ├── clusters/
│       │   └── {cluster_id}/
│       │       ├── context/
│       │       │   └── {agent_id}-{iteration}.json
│       │       ├── outputs/
│       │       │   └── {agent_id}-{iteration}.json
│       │       ├── logs/
│       │       │   └── {agent_id}.log
│       │       └── artifacts/
│       │           └── {filename}
│       └── repos/
│           └── {repo_hash}/
│               └── bundle.git      # Git bundle for repo
├── templates/
│   └── {template_id}.json
└── rootfs/
    └── zeroshot-agent-v{version}.ext4
```

---

## 6. Temporal Workflows

### 6.1 Cluster Workflow

```typescript
import {
  proxyActivities,
  defineSignal,
  defineQuery,
  setHandler,
  condition,
  startChild,
  CancellationScope,
} from '@temporalio/workflow';

// Activity interfaces
const { fetchIssue, classifyTask, resolveTemplate, persistCluster, publishEvent } = proxyActivities<
  typeof activities
>({
  startToCloseTimeout: '30s',
  retry: { maximumAttempts: 3 },
});

// Signals
export const stopSignal = defineSignal('stop');
export const resumeSignal = defineSignal<[string?]>('resume');

// Queries
export const getStateQuery = defineQuery<ClusterState>('getState');

// Workflow
export async function clusterWorkflow(input: ClusterInput): Promise<ClusterResult> {
  // State
  let state: ClusterState = 'initializing';
  let agents: Map<string, ChildWorkflowHandle> = new Map();
  let stopRequested = false;
  let resumePrompt: string | undefined;

  // Signal handlers
  setHandler(stopSignal, () => {
    stopRequested = true;
  });
  setHandler(resumeSignal, (prompt) => {
    resumePrompt = prompt;
  });
  setHandler(getStateQuery, () => state);

  try {
    // 1. Fetch issue content
    const issue = await fetchIssue(input.issueSource, input.issueIdentifier);

    // 2. Persist initial cluster
    await persistCluster({
      id: input.clusterId,
      tenantId: input.tenantId,
      state: 'initializing',
      issue,
    });

    // 3. Publish ISSUE_OPENED
    await publishEvent(input.clusterId, {
      topic: 'ISSUE_OPENED',
      sender: 'system',
      content: { text: issue.body, data: issue },
    });

    // 4. Run conductor for classification
    const classification = await classifyTask(issue);

    // 5. Resolve template
    const template = await resolveTemplate(classification.base, classification.params);

    // 6. Spawn agent workflows
    state = 'running';
    await persistCluster({ id: input.clusterId, state });

    for (const agentConfig of template.agents) {
      const handle = await startChild(agentWorkflow, {
        workflowId: `${input.clusterId}-${agentConfig.id}`,
        args: [
          {
            clusterId: input.clusterId,
            tenantId: input.tenantId,
            agentId: agentConfig.id,
            config: agentConfig,
          },
        ],
      });
      agents.set(agentConfig.id, handle);
    }

    // 7. Wait for completion or stop signal
    const completionPromise = waitForClusterCompletion(input.clusterId);
    const stopPromise = condition(() => stopRequested);

    const result = await Promise.race([
      completionPromise.then(() => 'completed' as const),
      stopPromise.then(() => 'stopped' as const),
    ]);

    // 8. Handle result
    if (result === 'stopped') {
      // Cancel all agents gracefully
      await CancellationScope.cancellable(async () => {
        for (const [id, handle] of agents) {
          await handle.cancel();
        }
      });
      state = 'stopped';
    } else {
      state = 'completed';
    }

    // 9. Finalize
    await persistCluster({
      id: input.clusterId,
      state,
      completedAt: new Date(),
    });

    return { state, clusterId: input.clusterId };
  } catch (error) {
    state = 'failed';
    await persistCluster({
      id: input.clusterId,
      state,
      error: error.message,
    });
    throw error;
  }
}
```

### 6.2 Agent Workflow

```typescript
export async function agentWorkflow(input: AgentInput): Promise<AgentResult> {
  const { clusterId, tenantId, agentId, config } = input;

  let iteration = 0;
  let state: AgentState = 'idle';

  // Query handler
  setHandler(getAgentStateQuery, () => ({ state, iteration }));

  while (iteration < config.maxIterations) {
    // 1. Wait for trigger
    state = 'waiting';
    const trigger = await waitForTrigger(clusterId, config.triggers);

    // 2. Evaluate trigger logic (if any)
    if (trigger.logic) {
      const shouldExecute = await evaluateTriggerLogic(clusterId, trigger, trigger.message);
      if (!shouldExecute) continue;
    }

    // 3. Build context
    state = 'building_context';
    const context = await buildAgentContext(clusterId, agentId, config);

    // 4. Execute in Docker/gVisor container
    state = 'executing';
    iteration++;

    const execution = await executeInContainer({
      clusterId,
      tenantId,
      agentId,
      iteration,
      context,
      config,
    });

    // 5. Process result
    const result = await parseAgentResult(execution.output, config.outputFormat);

    // 6. Execute hooks
    await executeAgentHooks(clusterId, agentId, config.hooks, result);

    // 7. Check for completion trigger
    if (trigger.action === 'stop_cluster') {
      await publishEvent(clusterId, {
        topic: 'CLUSTER_COMPLETE',
        sender: agentId,
        content: { text: 'Task completed', data: result },
      });
      break;
    }

    state = 'idle';
  }

  // Max iterations reached
  if (iteration >= config.maxIterations) {
    await publishEvent(clusterId, {
      topic: 'CLUSTER_FAILED',
      sender: agentId,
      content: { text: 'Max iterations reached', data: { iteration } },
    });
  }

  return { agentId, iterations: iteration, finalState: state };
}
```

### 6.3 Activity Implementations

```typescript
// activities/fetchIssue.ts
export async function fetchIssue(source: string, identifier: string): Promise<Issue> {
  const provider = getIssueProvider(source);
  return provider.fetchIssue(identifier);
}

// activities/executeInContainer.ts
export async function executeInContainer(input: ContainerInput): Promise<ContainerResult> {
  const nomadClient = getNomadClient();

  // 1. Upload context to S3
  const contextUrl = await uploadContext(input);

  // 2. Create Nomad job (Docker with gVisor runtime)
  const jobSpec = buildDockerJobSpec(input, contextUrl);
  const { EvalID } = await nomadClient.jobs.create(jobSpec);

  // 3. Wait for allocation
  const allocation = await waitForAllocation(EvalID, input.config.timeout);

  // 4. Stream logs to Redis
  const logStream = nomadClient.allocations.logs(allocation.ID, 'agent');
  for await (const chunk of logStream) {
    await publishToStream(input.clusterId, {
      type: 'AGENT_OUTPUT',
      agentId: input.agentId,
      data: chunk,
    });
  }

  // 5. Get final output
  const output = await getJobOutput(allocation.ID);

  // 6. Cleanup
  await nomadClient.jobs.delete(jobSpec.ID);

  return {
    output,
    tokenUsage: parseTokenUsage(output),
    duration: allocation.TaskStates.agent.FinishedAt - allocation.TaskStates.agent.StartedAt,
  };
}

// activities/publishEvent.ts
export async function publishEvent(clusterId: string, event: ClusterEvent): Promise<string> {
  const redis = getRedisClient();
  const db = getDbClient();

  const message = {
    id: generateMessageId(),
    timestamp: Date.now(),
    cluster_id: clusterId,
    ...event,
  };

  // 1. Persist to PostgreSQL (source of truth)
  await db.messages.create({ data: message });

  // 2. Publish to Redis Stream (real-time)
  const streamId = await redis.xadd(
    `cluster:${clusterId}:events`,
    '*',
    'data',
    JSON.stringify(message)
  );

  return message.id;
}
```

---

## 7. Agent Execution Runtime

### 7.1 Docker + gVisor Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                        Host System (Linux)                           │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │                     Nomad Client                               │  │
│  │  ┌─────────────────────────────────────────────────────────┐  │  │
│  │  │                  Docker Driver                           │  │  │
│  │  └─────────────────────────┬───────────────────────────────┘  │  │
│  └────────────────────────────┼──────────────────────────────────┘  │
│                               │                                      │
│  ┌────────────────────────────▼──────────────────────────────────┐  │
│  │                    Docker Engine                               │  │
│  │  ┌─────────────────────────────────────────────────────────┐  │  │
│  │  │              gVisor (runsc) Runtime                      │  │  │
│  │  │  ┌───────────────────────────────────────────────────┐  │  │  │
│  │  │  │           Sentry (User-space Kernel)              │  │  │  │
│  │  │  │  ┌─────────────────────────────────────────────┐  │  │  │  │
│  │  │  │  │            OCI Container                     │  │  │  │  │
│  │  │  │  │  ┌─────────────────────────────────────┐    │  │  │  │  │
│  │  │  │  │  │         Claude CLI (ct)              │    │  │  │  │  │
│  │  │  │  │  │    - Read /app/context.json          │    │  │  │  │  │
│  │  │  │  │  │    - Execute task                    │    │  │  │  │  │
│  │  │  │  │  │    - Stream output to stdout         │    │  │  │  │  │
│  │  │  │  │  │    - Write result to /app/output     │    │  │  │  │  │
│  │  │  │  │  └─────────────────────────────────────┘    │  │  │  │  │
│  │  │  │  │                                              │  │  │  │  │
│  │  │  │  │  /app/context.json  (injected via Nomad)    │  │  │  │  │
│  │  │  │  │  /app/output.json   (result)                │  │  │  │  │
│  │  │  │  │  /workspace/        (git repo clone)        │  │  │  │  │
│  │  │  │  └─────────────────────────────────────────────┘  │  │  │  │
│  │  │  │                                                    │  │  │  │
│  │  │  │  Syscall interception via ptrace/KVM              │  │  │  │
│  │  │  │  Gofer: Isolated filesystem proxy                 │  │  │  │
│  │  │  └───────────────────────────────────────────────────┘  │  │  │
│  │  └─────────────────────────────────────────────────────────┘  │  │
│  └────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────┘
```

### 7.2 Nomad Job Specification

```hcl
job "agent-${cluster_id}-${agent_id}" {
  type = "batch"

  meta {
    cluster_id = "${cluster_id}"
    agent_id   = "${agent_id}"
    tenant_id  = "${tenant_id}"
  }

  group "agent" {
    task "execute" {
      driver = "docker"

      config {
        image   = "zeroshot/agent:${version}"
        runtime = "runsc"  # gVisor runtime

        # Security settings
        readonly_rootfs = false
        cap_drop        = ["ALL"]
        cap_add         = ["NET_BIND_SERVICE"]

        # Resource limits enforced by Docker
        cpu_hard_limit    = true
        memory_hard_limit = true

        # Networking
        network_mode = "bridge"

        # Mount context and workspace
        volumes = [
          "local/context.json:/app/context.json:ro",
          "local/workspace:/workspace"
        ]

        # Labels for observability
        labels = [
          "cluster_id=${cluster_id}",
          "agent_id=${agent_id}",
          "tenant_id=${tenant_id}"
        ]

        # gVisor-specific flags
        runtime_args = [
          "--network=sandbox",
          "--overlay"
        ]
      }

      # Inject context
      template {
        destination = "local/context.json"
        data        = <<EOF
${context_json}
EOF
      }

      # Inject workspace (git repo)
      artifact {
        source      = "${workspace_artifact_url}"
        destination = "local/workspace"
      }

      # Environment from Vault
      template {
        destination = "secrets/env"
        env         = true
        data        = <<EOF
{{ with secret "kv/data/tenants/${tenant_id}/credentials" }}
ANTHROPIC_API_KEY={{ .Data.data.anthropic_api_key }}
{{ end }}
CLUSTER_ID=${cluster_id}
AGENT_ID=${agent_id}
REDIS_URL={{ env "REDIS_URL" }}
S3_ENDPOINT={{ env "S3_ENDPOINT" }}
EOF
      }

      resources {
        cpu    = 2000
        memory = 4096
      }

      # Timeout
      kill_timeout = "5m"

      # Liveness
      restart {
        attempts = 0  # No restart for batch jobs
      }
    }
  }
}
```

### 7.3 Agent Container Image

```dockerfile
# Dockerfile for agent container (runs under gVisor)
FROM alpine:3.19

# Install runtime dependencies
RUN apk add --no-cache \
    nodejs \
    npm \
    git \
    openssh-client \
    curl \
    jq

# Install Claude CLI
RUN npm install -g @anthropic-ai/claude-cli

# Install agent runtime
COPY agent-runtime /app/
WORKDIR /app

# Entry point
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

ENTRYPOINT ["/entrypoint.sh"]
```

```bash
#!/bin/sh
# entrypoint.sh

# Load context
CONTEXT=$(cat /app/context.json)
PROMPT=$(echo "$CONTEXT" | jq -r '.prompt')
SYSTEM=$(echo "$CONTEXT" | jq -r '.system')

# Clone repo if needed
if [ -n "$REPO_URL" ]; then
  git clone --depth=1 "$REPO_URL" /workspace
  cd /workspace
fi

# Execute Claude task
ct --system "$SYSTEM" \
   --output-format json \
   --no-interactive \
   "$PROMPT" \
   2>&1 | tee /app/output.log

# Extract result
node /app/parse-output.js /app/output.log > /app/output.json

# Upload result to S3
aws s3 cp /app/output.json "s3://${S3_BUCKET}/tenants/${TENANT_ID}/clusters/${CLUSTER_ID}/outputs/${AGENT_ID}.json"

# Signal completion
curl -X POST "${CALLBACK_URL}" \
  -H "Content-Type: application/json" \
  -d @/app/output.json
```

### 7.4 Resource Limits & Quotas

| Tier            | vCPUs | Memory | Disk | Network  | Timeout |
| --------------- | ----- | ------ | ---- | -------- | ------- |
| level1 (Haiku)  | 1     | 2GB    | 5GB  | 10 Mbps  | 5 min   |
| level2 (Sonnet) | 2     | 4GB    | 10GB | 50 Mbps  | 15 min  |
| level3 (Opus)   | 4     | 8GB    | 20GB | 100 Mbps | 30 min  |

---

## 8. API Design

### 8.1 REST API Specification

```yaml
openapi: 3.1.0
info:
  title: Zeroshot SaaS API
  version: 1.0.0

servers:
  - url: https://api.zeroshot.dev/v1

security:
  - bearerAuth: []
  - apiKeyAuth: []

paths:
  /clusters:
    post:
      summary: Create a new cluster
      operationId: createCluster
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/CreateClusterRequest'
      responses:
        '201':
          description: Cluster created
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/Cluster'

    get:
      summary: List clusters
      operationId: listClusters
      parameters:
        - name: state
          in: query
          schema:
            type: string
            enum: [pending, running, completed, failed, stopped]
        - name: limit
          in: query
          schema:
            type: integer
            default: 20
        - name: cursor
          in: query
          schema:
            type: string
      responses:
        '200':
          description: Cluster list
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/ClusterList'

  /clusters/{clusterId}:
    get:
      summary: Get cluster details
      operationId: getCluster
      parameters:
        - $ref: '#/components/parameters/clusterId'
      responses:
        '200':
          description: Cluster details
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/ClusterDetails'

    delete:
      summary: Stop/kill a cluster
      operationId: deleteCluster
      parameters:
        - $ref: '#/components/parameters/clusterId'
        - name: force
          in: query
          schema:
            type: boolean
            default: false
      responses:
        '204':
          description: Cluster stopped

  /clusters/{clusterId}/resume:
    post:
      summary: Resume a stopped/failed cluster
      operationId: resumeCluster
      parameters:
        - $ref: '#/components/parameters/clusterId'
      requestBody:
        content:
          application/json:
            schema:
              type: object
              properties:
                prompt:
                  type: string
                  description: Additional context for resume
      responses:
        '200':
          description: Cluster resumed
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/Cluster'

  /clusters/{clusterId}/messages:
    get:
      summary: Query cluster messages (ledger)
      operationId: getMessages
      parameters:
        - $ref: '#/components/parameters/clusterId'
        - name: topic
          in: query
          schema:
            type: string
        - name: sender
          in: query
          schema:
            type: string
        - name: since
          in: query
          schema:
            type: integer
            description: Unix timestamp
        - name: limit
          in: query
          schema:
            type: integer
            default: 100
      responses:
        '200':
          description: Message list
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/MessageList'

  /clusters/{clusterId}/logs:
    get:
      summary: Stream cluster logs (SSE)
      operationId: streamLogs
      parameters:
        - $ref: '#/components/parameters/clusterId'
        - name: since
          in: query
          schema:
            type: string
            description: Stream cursor
      responses:
        '200':
          description: Event stream
          content:
            text/event-stream:
              schema:
                type: string

components:
  securitySchemes:
    bearerAuth:
      type: http
      scheme: bearer
      bearerFormat: JWT
    apiKeyAuth:
      type: apiKey
      in: header
      name: X-API-Key

  parameters:
    clusterId:
      name: clusterId
      in: path
      required: true
      schema:
        type: string

  schemas:
    CreateClusterRequest:
      type: object
      required:
        - issue
      properties:
        issue:
          type: string
          description: Issue URL or identifier
        issueSource:
          type: string
          enum: [github, gitlab, jira, azure-devops, gitea, beads, text]
        template:
          type: string
          description: Template name override
        templateParams:
          type: object
          additionalProperties: true
        options:
          type: object
          properties:
            createPR:
              type: boolean
            autoMerge:
              type: boolean
        idempotencyKey:
          type: string
          description: Client-provided idempotency key

    Cluster:
      type: object
      properties:
        id:
          type: string
        state:
          type: string
          enum: [pending, initializing, running, stopping, stopped, completed, failed, killed]
        issueSource:
          type: string
        issueIdentifier:
          type: string
        issueTitle:
          type: string
        templateName:
          type: string
        workflowId:
          type: string
        createdAt:
          type: string
          format: date-time
        startedAt:
          type: string
          format: date-time
        completedAt:
          type: string
          format: date-time

    ClusterDetails:
      allOf:
        - $ref: '#/components/schemas/Cluster'
        - type: object
          properties:
            agents:
              type: array
              items:
                $ref: '#/components/schemas/Agent'
            tokenUsage:
              $ref: '#/components/schemas/TokenUsage'

    Agent:
      type: object
      properties:
        id:
          type: string
        role:
          type: string
        modelLevel:
          type: string
        state:
          type: string
        iteration:
          type: integer

    TokenUsage:
      type: object
      properties:
        inputTokens:
          type: integer
        outputTokens:
          type: integer
        totalCostUsd:
          type: number

    Message:
      type: object
      properties:
        id:
          type: string
        timestamp:
          type: integer
        topic:
          type: string
        sender:
          type: string
        receiver:
          type: string
        contentText:
          type: string
        contentData:
          type: object

    ClusterList:
      type: object
      properties:
        items:
          type: array
          items:
            $ref: '#/components/schemas/Cluster'
        nextCursor:
          type: string

    MessageList:
      type: object
      properties:
        items:
          type: array
          items:
            $ref: '#/components/schemas/Message'
        nextCursor:
          type: string
```

### 8.2 WebSocket API

```typescript
// Connection URL
//api.zeroshot.dev/v1/ws?token=<jwt_or_api_key>

// Client → Server Messages
wss: interface SubscribeMessage {
  type: 'subscribe';
  clusterId: string;
}

interface UnsubscribeMessage {
  type: 'unsubscribe';
  clusterId: string;
}

interface PingMessage {
  type: 'ping';
}

// Server → Client Messages
interface AgentOutputEvent {
  type: 'agent_output';
  clusterId: string;
  agentId: string;
  data: string;
  timestamp: number;
}

interface ClusterStateEvent {
  type: 'cluster_state';
  clusterId: string;
  state: ClusterState;
  timestamp: number;
}

interface AgentStateEvent {
  type: 'agent_state';
  clusterId: string;
  agentId: string;
  state: AgentState;
  iteration: number;
  timestamp: number;
}

interface MessageEvent {
  type: 'message';
  clusterId: string;
  message: Message;
}

interface ErrorEvent {
  type: 'error';
  code: string;
  message: string;
}

interface PongEvent {
  type: 'pong';
}
```

### 8.3 Webhook Events

```typescript
// Webhook payload
interface WebhookPayload {
  id: string;
  type: WebhookEventType;
  timestamp: string;
  data: ClusterEvent | AgentEvent;
  signature: string; // HMAC-SHA256
}

type WebhookEventType =
  | 'cluster.created'
  | 'cluster.started'
  | 'cluster.completed'
  | 'cluster.failed'
  | 'cluster.stopped'
  | 'agent.started'
  | 'agent.completed'
  | 'agent.failed';

// Webhook configuration
interface WebhookConfig {
  url: string;
  secret: string;
  events: WebhookEventType[];
  active: boolean;
}
```

---

## 9. Multi-Tenancy

### 9.1 Tenant Model

```typescript
interface Tenant {
  id: string;
  slug: string;
  name: string;
  plan: 'free' | 'pro' | 'enterprise';
  settings: TenantSettings;
  limits: TenantLimits;
}

interface TenantSettings {
  defaultIssueSource?: string;
  defaultTemplate?: string;
  maxModel: 'haiku' | 'sonnet' | 'opus';
  webhooks: WebhookConfig[];
  apiKeyRotationDays: number;
}

interface TenantLimits {
  maxConcurrentClusters: number;
  maxMonthlyTokens: number;
  maxStorageGB: number;
  rateLimitPerMinute: number;
}

// Plan limits
const PLAN_LIMITS: Record<string, TenantLimits> = {
  free: {
    maxConcurrentClusters: 1,
    maxMonthlyTokens: 100_000,
    maxStorageGB: 1,
    rateLimitPerMinute: 10,
  },
  pro: {
    maxConcurrentClusters: 5,
    maxMonthlyTokens: 1_000_000,
    maxStorageGB: 10,
    rateLimitPerMinute: 60,
  },
  enterprise: {
    maxConcurrentClusters: 50,
    maxMonthlyTokens: 10_000_000,
    maxStorageGB: 100,
    rateLimitPerMinute: 300,
  },
};
```

### 9.2 Tenant Context Propagation

```typescript
// Middleware: Extract and validate tenant
async function tenantMiddleware(req: Request, res: Response, next: NextFunction) {
  const auth = await validateAuth(req);

  // Set tenant context for RLS
  await db.$executeRaw`SELECT set_config('app.tenant_id', ${auth.tenantId}, true)`;

  // Attach to request
  req.tenant = await db.tenants.findUnique({ where: { id: auth.tenantId } });
  req.user = auth.user;

  next();
}

// Service layer: Context always available
class ClusterService {
  constructor(private ctx: TenantContext) {}

  async create(input: CreateClusterInput): Promise<Cluster> {
    // Check limits
    const current = await this.countRunning();
    if (current >= this.ctx.tenant.limits.maxConcurrentClusters) {
      throw new LimitExceededError('concurrent_clusters');
    }

    // Create with tenant_id (RLS handles isolation)
    return db.clusters.create({
      data: {
        ...input,
        tenant_id: this.ctx.tenant.id,
        created_by: this.ctx.user.id,
      },
    });
  }
}
```

### 9.3 Rate Limiting

```typescript
// Redis-based sliding window rate limiter
async function checkRateLimit(tenantId: string, limit: number): Promise<boolean> {
  const key = `ratelimit:${tenantId}:api`;
  const now = Date.now();
  const window = 60_000; // 1 minute

  const multi = redis.multi();
  multi.zremrangebyscore(key, 0, now - window);
  multi.zadd(key, now, `${now}-${Math.random()}`);
  multi.zcard(key);
  multi.pexpire(key, window);

  const results = await multi.exec();
  const count = results[2][1] as number;

  return count <= limit;
}

// Middleware
async function rateLimitMiddleware(req: Request, res: Response, next: NextFunction) {
  const limit = req.tenant.limits.rateLimitPerMinute;

  if (!(await checkRateLimit(req.tenant.id, limit))) {
    res.status(429).json({
      error: 'rate_limit_exceeded',
      retryAfter: 60,
    });
    return;
  }

  next();
}
```

### 9.4 Credential Management

```typescript
// Vault path structure
// kv/data/tenants/{tenant_id}/credentials
//   - anthropic_api_key
//   - openai_api_key
//   - github_token
//   - ...

// Credential service
class CredentialService {
  private vault: VaultClient;

  async getCredential(tenantId: string, key: string): Promise<string | null> {
    const path = `kv/data/tenants/${tenantId}/credentials`;
    const secret = await this.vault.read(path);
    return secret?.data?.data?.[key] ?? null;
  }

  async setCredential(tenantId: string, key: string, value: string): Promise<void> {
    const path = `kv/data/tenants/${tenantId}/credentials`;

    // Read existing
    const existing = await this.vault.read(path);
    const data = existing?.data?.data ?? {};

    // Update
    data[key] = value;
    await this.vault.write(path, { data });
  }

  // For Nomad jobs: generate short-lived token
  async generateAgentToken(tenantId: string, clusterId: string): Promise<string> {
    const policy = `
      path "kv/data/tenants/${tenantId}/credentials" {
        capabilities = ["read"]
      }
    `;

    return this.vault.createToken({
      policies: [`agent-${tenantId}`],
      ttl: '1h',
      metadata: { clusterId },
    });
  }
}
```

---

## 10. Security Architecture

### 10.1 Authentication

```
┌─────────────────────────────────────────────────────────────────────┐
│                      Authentication Flow                             │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌──────────┐     ┌──────────┐     ┌──────────┐     ┌──────────┐   │
│  │  Client  │────▶│   API    │────▶│   Auth   │────▶│   JWT    │   │
│  │          │     │ Gateway  │     │ Service  │     │ Validation│   │
│  └──────────┘     └──────────┘     └──────────┘     └──────────┘   │
│       │                                                   │          │
│       │                                                   ▼          │
│       │           ┌──────────────────────────────────────────┐      │
│       │           │            Token Types                    │      │
│       │           ├──────────────────────────────────────────┤      │
│       │           │ JWT (user sessions)    - 1 hour TTL      │      │
│       │           │ API Key (service auth) - No expiry       │      │
│       │           │ Refresh Token          - 30 day TTL      │      │
│       │           └──────────────────────────────────────────┘      │
│       │                                                              │
│       ▼                                                              │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │                    OAuth2/OIDC Providers                      │   │
│  │  ┌────────┐  ┌────────┐  ┌────────┐  ┌────────┐  ┌────────┐ │   │
│  │  │ Google │  │ GitHub │  │ GitLab │  │  SAML  │  │  OIDC  │ │   │
│  │  └────────┘  └────────┘  └────────┘  └────────┘  └────────┘ │   │
│  └──────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

### 10.2 Authorization (RBAC)

```typescript
// Roles
enum Role {
  OWNER = 'owner',
  ADMIN = 'admin',
  MEMBER = 'member',
  VIEWER = 'viewer',
}

// Permissions
const PERMISSIONS: Record<Role, string[]> = {
  owner: ['*'],
  admin: [
    'clusters:*',
    'agents:*',
    'templates:*',
    'webhooks:*',
    'members:read',
    'members:invite',
    'settings:read',
  ],
  member: [
    'clusters:create',
    'clusters:read',
    'clusters:update',
    'clusters:delete',
    'agents:read',
    'templates:read',
  ],
  viewer: ['clusters:read', 'agents:read'],
};

// Authorization middleware
function authorize(permission: string) {
  return (req: Request, res: Response, next: NextFunction) => {
    const userPerms = PERMISSIONS[req.user.role];

    if (!userPerms.includes('*') && !userPerms.includes(permission)) {
      return res.status(403).json({ error: 'forbidden' });
    }

    next();
  };
}
```

### 10.3 gVisor Container Isolation Security

```
┌─────────────────────────────────────────────────────────────────────┐
│                      gVisor Security Layers                          │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Layer 1: User-Space Kernel (Sentry)                                │
│  ├── Intercepts all syscalls from application                       │
│  ├── Implements Linux syscall interface in Go (memory-safe)         │
│  ├── No direct host kernel access from container                    │
│  └── Only ~70 syscalls exposed to host (vs 300+ for runc)           │
│                                                                      │
│  Layer 2: Filesystem Isolation (Gofer)                              │
│  ├── Separate process handles all filesystem operations             │
│  ├── 9P protocol between Sentry and Gofer                           │
│  ├── Gofer runs with minimal privileges                             │
│  └── Filesystem access fully mediated                               │
│                                                                      │
│  Layer 3: Platform Isolation (ptrace or KVM)                        │
│  ├── ptrace: Pure user-space, no kernel modules                     │
│  ├── KVM: Hardware-assisted for better performance                  │
│  ├── Seccomp filter on Sentry (minimal host syscalls)               │
│  └── Namespaces (network, PID, mount, user)                         │
│                                                                      │
│  Layer 4: Network Isolation                                         │
│  ├── gVisor netstack (user-space network stack)                     │
│  ├── No direct host network access                                  │
│  ├── Egress only to allowed hosts (Anthropic API, S3)               │
│  └── mTLS for all internal communication                            │
│                                                                      │
│  Layer 5: Docker/OCI Integration                                    │
│  ├── Standard OCI runtime interface                                 │
│  ├── Works with existing Docker tooling                             │
│  ├── Image scanning and registry security                           │
│  └── Cgroups v2 for resource limits                                 │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### 10.4 Secrets Management

```hcl
# Vault policy for Nomad jobs
path "kv/data/tenants/{{identity.entity.metadata.tenant_id}}/credentials" {
  capabilities = ["read"]
}

path "pki/issue/agent" {
  capabilities = ["create", "update"]
}

# Vault policy for API service
path "kv/data/tenants/*" {
  capabilities = ["read", "create", "update", "delete"]
}

path "transit/encrypt/tenant-secrets" {
  capabilities = ["update"]
}

path "transit/decrypt/tenant-secrets" {
  capabilities = ["update"]
}
```

### 10.5 Audit Logging

```typescript
interface AuditEvent {
  id: string;
  timestamp: string;
  tenantId: string;
  userId: string;
  action: string;
  resource: string;
  resourceId: string;
  result: 'success' | 'failure';
  ip: string;
  userAgent: string;
  details: Record<string, unknown>;
}

// Audit middleware
async function auditMiddleware(req: Request, res: Response, next: NextFunction) {
  const startTime = Date.now();

  res.on('finish', async () => {
    const event: AuditEvent = {
      id: generateId(),
      timestamp: new Date().toISOString(),
      tenantId: req.tenant?.id,
      userId: req.user?.id,
      action: `${req.method} ${req.route?.path}`,
      resource: extractResource(req),
      resourceId: req.params.id,
      result: res.statusCode < 400 ? 'success' : 'failure',
      ip: req.ip,
      userAgent: req.headers['user-agent'],
      details: {
        statusCode: res.statusCode,
        duration: Date.now() - startTime,
        requestId: req.headers['x-request-id'],
      },
    };

    // Write to append-only audit log
    await auditLog.append(event);
  });

  next();
}
```

---

## 11. Observability

### 11.1 Metrics (Prometheus)

```typescript
// Key metrics
const metrics = {
  // Cluster metrics
  clusters_created_total: new Counter({
    name: 'zeroshot_clusters_created_total',
    help: 'Total clusters created',
    labelNames: ['tenant_id', 'template', 'issue_source'],
  }),

  clusters_active: new Gauge({
    name: 'zeroshot_clusters_active',
    help: 'Currently active clusters',
    labelNames: ['tenant_id', 'state'],
  }),

  cluster_duration_seconds: new Histogram({
    name: 'zeroshot_cluster_duration_seconds',
    help: 'Cluster execution duration',
    labelNames: ['tenant_id', 'template', 'result'],
    buckets: [60, 300, 600, 1800, 3600],
  }),

  // Agent metrics
  agent_executions_total: new Counter({
    name: 'zeroshot_agent_executions_total',
    help: 'Total agent executions',
    labelNames: ['tenant_id', 'role', 'model_level'],
  }),

  agent_execution_duration_seconds: new Histogram({
    name: 'zeroshot_agent_execution_duration_seconds',
    help: 'Agent execution duration',
    labelNames: ['role', 'model_level'],
    buckets: [10, 30, 60, 120, 300, 600],
  }),

  // Token metrics
  tokens_consumed_total: new Counter({
    name: 'zeroshot_tokens_consumed_total',
    help: 'Total tokens consumed',
    labelNames: ['tenant_id', 'model', 'type'], // type: input/output
  }),

  // Infrastructure metrics
  containers_active: new Gauge({
    name: 'zeroshot_containers_active',
    help: 'Currently running gVisor containers',
    labelNames: ['host'],
  }),

  redis_stream_lag: new Gauge({
    name: 'zeroshot_redis_stream_lag',
    help: 'Redis stream consumer lag',
    labelNames: ['stream', 'consumer_group'],
  }),
};
```

### 11.2 Logging (Structured)

```typescript
// Logger configuration
const logger = pino({
  level: process.env.LOG_LEVEL || 'info',
  formatters: {
    level: (label) => ({ level: label }),
  },
  base: {
    service: 'zeroshot-api',
    version: process.env.VERSION,
    environment: process.env.NODE_ENV,
  },
});

// Request logging
app.use((req, res, next) => {
  const requestId = req.headers['x-request-id'] || generateId();

  req.log = logger.child({
    requestId,
    tenantId: req.tenant?.id,
    userId: req.user?.id,
    method: req.method,
    path: req.path,
  });

  req.log.info('request_started');

  res.on('finish', () => {
    req.log.info(
      {
        statusCode: res.statusCode,
        duration: res.get('X-Response-Time'),
      },
      'request_completed'
    );
  });

  next();
});
```

### 11.3 Distributed Tracing (OpenTelemetry)

```typescript
import { trace, SpanStatusCode } from '@opentelemetry/api';

const tracer = trace.getTracer('zeroshot-api');

// Instrument cluster creation
async function createCluster(input: CreateClusterInput): Promise<Cluster> {
  return tracer.startActiveSpan('cluster.create', async (span) => {
    span.setAttributes({
      'tenant.id': input.tenantId,
      'issue.source': input.issueSource,
      'issue.identifier': input.issueIdentifier,
    });

    try {
      const cluster = await db.clusters.create({ data: input });

      span.setAttributes({
        'cluster.id': cluster.id,
        'cluster.template': cluster.templateName,
      });

      // Start Temporal workflow (creates child span)
      await temporalClient.workflow.start(clusterWorkflow, {
        workflowId: cluster.id,
        args: [{ clusterId: cluster.id, ...input }],
      });

      span.setStatus({ code: SpanStatusCode.OK });
      return cluster;
    } catch (error) {
      span.setStatus({
        code: SpanStatusCode.ERROR,
        message: error.message,
      });
      span.recordException(error);
      throw error;
    } finally {
      span.end();
    }
  });
}
```

### 11.4 Dashboard (Grafana)

Key dashboards:

1. **System Overview** - Cluster throughput, success rate, latency
2. **Tenant Health** - Per-tenant usage, limits, errors
3. **Agent Performance** - Execution time by role/model, token efficiency
4. **Infrastructure** - Nomad jobs, gVisor containers, Redis streams
5. **Cost Analysis** - Token usage by tenant, model distribution

---

## 12. Deployment Topology

### 12.1 Production Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                          Region: us-east-1                           │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │                      VPC: zeroshot-prod                      │    │
│  │                                                               │    │
│  │  ┌──────────────────────────────────────────────────────┐    │    │
│  │  │                 Public Subnet                         │    │    │
│  │  │  ┌────────────────┐  ┌────────────────┐              │    │    │
│  │  │  │  ALB (API)     │  │  NLB (WS)      │              │    │    │
│  │  │  └───────┬────────┘  └───────┬────────┘              │    │    │
│  │  └──────────┼───────────────────┼────────────────────────┘    │    │
│  │             │                   │                              │    │
│  │  ┌──────────┼───────────────────┼────────────────────────┐    │    │
│  │  │          │   Private Subnet  │                         │    │    │
│  │  │          ▼                   ▼                         │    │    │
│  │  │  ┌────────────────────────────────────────────────┐   │    │    │
│  │  │  │              EKS Cluster (Services)             │   │    │    │
│  │  │  │  ┌─────────┐ ┌─────────┐ ┌─────────┐           │   │    │    │
│  │  │  │  │ API x3  │ │ WS x3   │ │Stream x2│           │   │    │    │
│  │  │  │  └─────────┘ └─────────┘ └─────────┘           │   │    │    │
│  │  │  │  ┌─────────────────────────────────┐           │   │    │    │
│  │  │  │  │     Temporal Workers x5         │           │   │    │    │
│  │  │  │  └─────────────────────────────────┘           │   │    │    │
│  │  │  └────────────────────────────────────────────────┘   │    │    │
│  │  │                                                        │    │    │
│  │  │  ┌────────────────────────────────────────────────┐   │    │    │
│  │  │  │           Nomad Cluster (Compute)               │   │    │    │
│  │  │  │  ┌─────────────────────────────────────────┐   │   │    │    │
│  │  │  │  │  Nomad Server x3 (m5.large)             │   │   │    │    │
│  │  │  │  └─────────────────────────────────────────┘   │   │    │    │
│  │  │  │  ┌─────────────────────────────────────────┐   │   │    │    │
│  │  │  │  │  Nomad Client x10 (m5.4xlarge)          │   │   │    │    │
│  │  │  │  │  - Docker + gVisor runtime               │   │   │    │    │
│  │  │  │  │  - 16 vCPUs, 64GB RAM each              │   │   │    │    │
│  │  │  │  └─────────────────────────────────────────┘   │   │    │    │
│  │  │  └────────────────────────────────────────────────┘   │    │    │
│  │  │                                                        │    │    │
│  │  │  ┌────────────────────────────────────────────────┐   │    │    │
│  │  │  │              Data Layer                         │   │    │    │
│  │  │  │  ┌─────────────┐  ┌─────────────┐              │   │    │    │
│  │  │  │  │ RDS Postgres│  │ ElastiCache │              │   │    │    │
│  │  │  │  │ (Multi-AZ)  │  │ Redis       │              │   │    │    │
│  │  │  │  └─────────────┘  └─────────────┘              │   │    │    │
│  │  │  │  ┌─────────────┐  ┌─────────────┐              │   │    │    │
│  │  │  │  │ S3 (MinIO)  │  │ Temporal    │              │   │    │    │
│  │  │  │  │             │  │ Server      │              │   │    │    │
│  │  │  │  └─────────────┘  └─────────────┘              │   │    │    │
│  │  │  └────────────────────────────────────────────────┘   │    │    │
│  │  └────────────────────────────────────────────────────────┘    │    │
│  │                                                               │    │
│  │  ┌──────────────────────────────────────────────────────┐    │    │
│  │  │              Observability (Shared VPC)               │    │    │
│  │  │  ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐    │    │    │
│  │  │  │Prometheus│ │ Grafana │ │  Loki   │ │  Tempo  │    │    │    │
│  │  │  └─────────┘ └─────────┘ └─────────┘ └─────────┘    │    │    │
│  │  └──────────────────────────────────────────────────────┘    │    │
│  └───────────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────────┘
```

### 12.2 Nomad Client Sizing

```
┌─────────────────────────────────────────────────────────────────────┐
│                  m5.4xlarge Instance Layout                          │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Total Resources: 16 vCPUs, 64GB RAM, EBS storage                   │
│                                                                      │
│  Reserved for Host + gVisor overhead: 2 vCPUs, 4GB RAM              │
│  Available for Containers: 14 vCPUs, 60GB RAM                       │
│                                                                      │
│  ┌────────────────────────────────────────────────────────────┐     │
│  │               Container Capacity (per host)                 │     │
│  ├────────────────┬───────────┬───────────┬──────────────────┤     │
│  │ Model Level    │ vCPUs     │ Memory    │ Max Containers   │     │
│  ├────────────────┼───────────┼───────────┼──────────────────┤     │
│  │ level1 (Haiku) │ 1         │ 2GB       │ 14               │     │
│  │ level2 (Sonnet)│ 2         │ 4GB       │ 7                │     │
│  │ level3 (Opus)  │ 4         │ 8GB       │ 3                │     │
│  └────────────────┴───────────┴───────────┴──────────────────┘     │
│                                                                      │
│  Mixed Workload Example (typical):                                  │
│  - 4 x level1 containers = 4 vCPUs, 8GB RAM                        │
│  - 3 x level2 containers = 6 vCPUs, 12GB RAM                       │
│  - 1 x level3 container  = 4 vCPUs, 8GB RAM                        │
│  - Remaining: 0 vCPUs, 32GB RAM (memory headroom)                  │
│                                                                      │
│  Note: gVisor overhead ~10-15% CPU, ~50MB RAM per container        │
│  Scale horizontally by adding more m5.4xlarge instances            │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### 12.3 Terraform Structure

```
terraform/
├── environments/
│   ├── prod/
│   │   ├── main.tf
│   │   ├── variables.tf
│   │   └── terraform.tfvars
│   ├── staging/
│   └── dev/
├── modules/
│   ├── vpc/
│   ├── eks/
│   ├── nomad/
│   ├── rds/
│   ├── elasticache/
│   ├── s3/
│   ├── temporal/
│   └── observability/
└── shared/
    ├── providers.tf
    └── backend.tf
```

---

## 13. Migration Strategy

### 13.1 Phase 1: Foundation (Weeks 1-4)

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Phase 1: Foundation                          │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Deliverables:                                                       │
│  □ PostgreSQL schema deployed                                        │
│  □ Redis cluster provisioned                                         │
│  □ S3 bucket configured                                              │
│  □ Vault cluster deployed                                            │
│  □ Basic API service (CRUD operations)                               │
│  □ Authentication (API keys only)                                    │
│  □ Single-tenant mode working                                        │
│                                                                      │
│  Data Migration:                                                     │
│  - Export existing SQLite ledgers                                    │
│  - Import to PostgreSQL                                              │
│  - Verify data integrity                                             │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### 13.2 Phase 2: Orchestration (Weeks 5-8)

```
┌─────────────────────────────────────────────────────────────────────┐
│                       Phase 2: Orchestration                         │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Deliverables:                                                       │
│  □ Temporal server deployed                                          │
│  □ ClusterWorkflow implemented                                       │
│  □ AgentWorkflow implemented                                         │
│  □ Conductor activity (classification)                               │
│  □ Redis Streams integration                                         │
│  □ WebSocket gateway (basic)                                         │
│                                                                      │
│  Behavior Parity:                                                    │
│  - Same classification logic                                         │
│  - Same template resolution                                          │
│  - Same hook execution                                               │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### 13.3 Phase 3: Isolation (Weeks 9-12)

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Phase 3: Isolation                           │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Deliverables:                                                       │
│  □ Nomad cluster deployed                                            │
│  □ gVisor (runsc) runtime installed on all Nomad clients            │
│  □ Agent container image built and tested                            │
│  □ Docker networking configured                                      │
│  □ Vault integration for credentials                                 │
│  □ S3 artifact upload/download                                       │
│                                                                      │
│  Security Validation:                                                │
│  - Container escape testing                                          │
│  - Syscall filtering verification                                    │
│  - Network isolation verification                                    │
│  - Credential leakage testing                                        │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### 13.4 Phase 4: Multi-Tenancy (Weeks 13-16)

```
┌─────────────────────────────────────────────────────────────────────┐
│                       Phase 4: Multi-Tenancy                         │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Deliverables:                                                       │
│  □ Tenant management API                                             │
│  □ Row-level security enabled                                        │
│  □ Rate limiting per tenant                                          │
│  □ Usage metering                                                    │
│  □ OAuth2/OIDC integration                                           │
│  □ RBAC implementation                                               │
│  □ Audit logging                                                     │
│                                                                      │
│  Compliance:                                                         │
│  - Data isolation verification                                       │
│  - Access control testing                                            │
│  - Audit trail verification                                          │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### 13.5 Phase 5: Production (Weeks 17-20)

```
┌─────────────────────────────────────────────────────────────────────┐
│                        Phase 5: Production                           │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Deliverables:                                                       │
│  □ Production infrastructure deployed                                │
│  □ Observability stack (Prometheus, Grafana, Loki, Tempo)           │
│  □ Alerting configured                                               │
│  □ Runbooks documented                                               │
│  □ Load testing completed                                            │
│  □ Security audit passed                                             │
│  □ Beta customer onboarding                                          │
│                                                                      │
│  SLOs:                                                               │
│  - API availability: 99.9%                                           │
│  - Cluster success rate: 95%                                         │
│  - P99 latency (API): <200ms                                         │
│  - P99 latency (cluster start): <5s                                  │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

---

## 14. 12-Factor Compliance

### 14.1 Checklist

| Factor                     | Requirement                     | Implementation                                      |
| -------------------------- | ------------------------------- | --------------------------------------------------- |
| **I. Codebase**            | One codebase, many deploys      | Monorepo with environment-specific config           |
| **II. Dependencies**       | Explicit declaration            | `package.json`, lockfiles, container images         |
| **III. Config**            | Store in environment            | All config via env vars, no hardcoded values        |
| **IV. Backing Services**   | Treat as attached resources     | PostgreSQL, Redis, S3 via connection strings        |
| **V. Build, Release, Run** | Strict separation               | CI builds image → CD deploys tag → Runtime executes |
| **VI. Processes**          | Stateless                       | All state in PostgreSQL/Redis, no local files       |
| **VII. Port Binding**      | Export via port                 | Each service binds to `$PORT`                       |
| **VIII. Concurrency**      | Scale via process model         | Horizontal scaling of stateless services            |
| **IX. Disposability**      | Fast startup, graceful shutdown | <10s startup, SIGTERM handling                      |
| **X. Dev/Prod Parity**     | Keep environments similar       | Docker Compose for local, same images in prod       |
| **XI. Logs**               | Treat as event streams          | JSON to stdout, collected by Loki                   |
| **XII. Admin Processes**   | Run as one-off processes        | Temporal activities, Nomad batch jobs               |

### 14.2 Environment Variables

```bash
# Required
DATABASE_URL=postgres://user:pass@host:5432/zeroshot
REDIS_URL=redis://host:6379
S3_ENDPOINT=https://s3.region.amazonaws.com
S3_BUCKET=zeroshot-artifacts
TEMPORAL_ADDRESS=temporal.internal:7233
VAULT_ADDR=https://vault.internal:8200
NOMAD_ADDR=https://nomad.internal:4646

# Authentication
JWT_SECRET=<32-byte-hex>
API_KEY_SALT=<32-byte-hex>

# Optional (with defaults)
PORT=3000
LOG_LEVEL=info
NODE_ENV=production
METRICS_PORT=9090

# Feature flags
ENABLE_MULTI_TENANCY=true
ENABLE_FIRECRACKER=true
MAX_CONCURRENT_CLUSTERS=100
```

---

## Appendix A: Technology Comparison

### A.1 Why Redis Streams over Kafka?

| Factor          | Redis Streams           | Kafka                    |
| --------------- | ----------------------- | ------------------------ |
| Complexity      | Lower ops burden        | Requires ZooKeeper/KRaft |
| Latency         | Sub-millisecond         | ~5ms typical             |
| Ordering        | Per-stream              | Per-partition            |
| Consumer Groups | Built-in                | Built-in                 |
| Persistence     | Configurable            | Always                   |
| Use Case Fit    | Event bus for real-time | Log aggregation at scale |

**Decision:** Redis Streams sufficient for expected throughput (<10k events/sec), simpler to operate.

### A.2 Why Nomad over Kubernetes?

| Factor                | Nomad                     | Kubernetes                    |
| --------------------- | ------------------------- | ----------------------------- |
| Docker/gVisor Support | Native Docker driver      | Native (with runtime config)  |
| Complexity            | Single binary             | Complex control plane         |
| Resource Overhead     | <100MB RAM                | >500MB RAM per node           |
| Scheduling Speed      | <100ms                    | ~seconds                      |
| Learning Curve        | Lower                     | Higher                        |
| Operational Burden    | Minimal (HashiCorp stack) | Significant (many components) |

**Decision:** Nomad's simplicity and tight integration with Consul/Vault outweigh Kubernetes ecosystem benefits for this use case. Both support gVisor equally well.

### A.3 Why gVisor over Standard Docker (runc)?

| Factor            | Docker + gVisor            | Docker + runc               |
| ----------------- | -------------------------- | --------------------------- |
| Isolation         | User-space kernel (Sentry) | Kernel (cgroups/namespaces) |
| Syscall Exposure  | ~70 syscalls to host       | 300+ syscalls to host       |
| Boot Time         | ~500ms                     | ~200ms                      |
| Memory Overhead   | ~50MB (Sentry + Gofer)     | ~10MB                       |
| Security          | Minimal attack surface     | Larger attack surface       |
| Escape Risk       | Low (syscall interception) | Higher (kernel shared)      |
| OCI Compatibility | Full (drop-in runtime)     | Native                      |
| Tooling           | Standard Docker/K8s/Nomad  | Standard Docker/K8s/Nomad   |

**Decision:** gVisor provides strong isolation with OCI compatibility. Trade-off: slightly higher overhead (~15% CPU, ~50MB RAM) for significantly reduced attack surface. Easier operations than Firecracker (no KVM, no custom images).

---

## Appendix B: Capacity Planning

### B.1 Scaling Thresholds

| Metric                         | Threshold         | Action              |
| ------------------------------ | ----------------- | ------------------- |
| API latency P99 > 500ms        | Scale API pods    | HPA target: 70% CPU |
| Redis memory > 80%             | Add Redis nodes   | Manual, then auto   |
| Nomad clients > 70% utilized   | Add compute nodes | ASG target: 70%     |
| PostgreSQL connections > 80%   | Add read replicas | PgBouncer first     |
| Temporal workflow latency > 1s | Scale workers     | Manual review       |

### B.2 Cost Estimates (Production)

| Component              | Instance Type          | Count | Monthly Cost   |
| ---------------------- | ---------------------- | ----- | -------------- |
| EKS Control Plane      | -                      | 1     | $72            |
| EKS Nodes              | m5.large               | 6     | $432           |
| Nomad Servers          | m5.large               | 3     | $216           |
| Nomad Clients (gVisor) | m5.4xlarge             | 10    | $5,530         |
| RDS PostgreSQL         | db.r5.large (Multi-AZ) | 1     | $350           |
| ElastiCache Redis      | cache.r5.large         | 3     | $450           |
| S3                     | -                      | -     | ~$100          |
| Data Transfer          | -                      | -     | ~$500          |
| **Total**              |                        |       | **~$7,650/mo** |

_Note: m5.4xlarge chosen for balance of CPU/memory. Can use m5.2xlarge ($2,765/mo for 10) for lower cost with fewer concurrent containers per host._

---

_Document End_
