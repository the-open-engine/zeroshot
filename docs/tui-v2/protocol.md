protocolVersion: 1

# TUI v2 JSON-RPC Protocol (v0)

## Framing

- Each message is framed as: `Content-Length: <N>\r\n\r\n` followed by `<N>` bytes of UTF-8 JSON.
- Ignore unknown headers.
- Reject frames larger than 10MB with RPC error `-32600` (invalid request).

## Envelope (JSON-RPC 2.0)

All messages are JSON-RPC 2.0 objects:

- `jsonrpc`: must be `"2.0"`.
- `id`: string or number (required for requests/responses, omitted for notifications).
- `method`: string (required for requests/notifications).
- `params`: optional; object | array | null.
- `result` or `error` (responses only; exactly one).

## Error Object

```json
{
  "code": -32600,
  "message": "Invalid request",
  "data": {
    "detail": "optional details",
    "fields": {
      "/params/clusterId": "must be string"
    }
  }
}
```

- `data.detail` is a human-readable summary (optional).
- `data.fields` maps JSON pointers to per-field messages (optional).

## Error Codes

- `-32700` parse error
- `-32600` invalid request
- `-32601` method not found
- `-32602` invalid params
- `-32603` internal error
- `-32000` protocol version mismatch
- `-32001` orchestrator unavailable
- `-32002` cluster not found
- `-32003` unsupported capability

## Domain Types

### ClusterSummary

```json
{
  "id": "cluster-123",
  "state": "running",
  "provider": "codex",
  "createdAt": 1769810000000,
  "agentCount": 3,
  "messageCount": 120,
  "cwd": "/path/to/workdir"
}
```

### ClusterMetrics

```json
{
  "id": "cluster-123",
  "supported": true,
  "cpuPercent": 12.3,
  "memoryMB": 256.7
}
```

### ClusterLogLine

```json
{
  "id": "line-1",
  "timestamp": 1769811111000,
  "text": "Agent output",
  "agent": "worker",
  "role": "implementation",
  "sender": "worker"
}
```

### TimelineEvent

```json
{
  "id": "evt-1",
  "timestamp": 1769811111000,
  "topic": "PLAN_READY",
  "label": "Plan ready",
  "approved": null,
  "sender": "planner"
}
```

### TopologyAgent / TopologyEdge / ClusterTopology

```json
{
  "agents": [{ "id": "worker", "role": "implementation" }],
  "edges": [
    { "from": "system", "to": "ISSUE_OPENED", "topic": "ISSUE_OPENED", "kind": "source" },
    { "from": "ISSUE_OPENED", "to": "worker", "topic": "ISSUE_OPENED", "kind": "trigger" }
  ],
  "topics": ["ISSUE_OPENED"]
}
```

### GuidanceDeliveryResult / ClusterGuidanceDelivery

```json
{
  "summary": { "injected": 1, "queued": 0, "total": 1 },
  "agents": {
    "worker": { "status": "injected", "reason": null, "method": "pty", "taskId": "task-1" }
  },
  "timestamp": 1769811111000
}
```

## Methods (Requests/Responses)

### initialize

Request:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": 1,
    "client": { "name": "zeroshot-tui", "version": "0.1.0", "pid": 12345 },
    "capabilities": { "wantsMetrics": true, "wantsTopology": false }
  }
}
```

Response:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "protocolVersion": 1,
    "server": { "name": "zeroshot-backend", "version": "5.4.0" },
    "capabilities": {
      "methods": ["initialize", "listClusters", "getClusterSummary", "unsubscribe"],
      "notifications": ["clusterLogLines", "clusterTimelineEvents"]
    }
  }
}
```

### listClusters

Request: `listClusters()`

Response:

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "clusters": [
      /* ClusterSummary[] */
    ]
  }
}
```

### getClusterSummary

Request:

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "getClusterSummary",
  "params": { "clusterId": "cluster-123" }
}
```

Response:

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "result": {
    "summary": {
      /* ClusterSummary */
    }
  }
}
```

### listClusterMetrics

Request:

```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "method": "listClusterMetrics",
  "params": { "clusterIds": ["cluster-123"] }
}
```

Response:

```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "result": {
    "metrics": [
      /* ClusterMetrics[] */
    ]
  }
}
```

### startClusterFromText

Request:

```json
{
  "jsonrpc": "2.0",
  "id": 5,
  "method": "startClusterFromText",
  "params": {
    "text": "Implement the requested feature",
    "providerOverride": "codex",
    "clusterId": "cluster-123"
  }
}
```

Response:

```json
{ "jsonrpc": "2.0", "id": 5, "result": { "clusterId": "cluster-123" } }
```

### startClusterFromIssue

Request:

```json
{
  "jsonrpc": "2.0",
  "id": 6,
  "method": "startClusterFromIssue",
  "params": { "ref": "covibes/zeroshot#240", "providerOverride": null }
}
```

Response:

```json
{ "jsonrpc": "2.0", "id": 6, "result": { "clusterId": "cluster-456" } }
```

### sendGuidanceToAgent

Request:

```json
{
  "jsonrpc": "2.0",
  "id": 7,
  "method": "sendGuidanceToAgent",
  "params": {
    "clusterId": "cluster-123",
    "agentId": "worker",
    "text": "Focus on tests",
    "timeoutMs": 5000
  }
}
```

Response:

```json
{
  "jsonrpc": "2.0",
  "id": 7,
  "result": {
    "result": {
      /* GuidanceDeliveryResult */
    }
  }
}
```

### sendGuidanceToCluster

Request:

```json
{
  "jsonrpc": "2.0",
  "id": 8,
  "method": "sendGuidanceToCluster",
  "params": { "clusterId": "cluster-123", "text": "Ship it", "timeoutMs": 5000 }
}
```

Response:

```json
{
  "jsonrpc": "2.0",
  "id": 8,
  "result": {
    "result": {
      /* ClusterGuidanceDelivery */
    }
  }
}
```

### subscribeClusterLogs

Request:

```json
{
  "jsonrpc": "2.0",
  "id": 9,
  "method": "subscribeClusterLogs",
  "params": { "clusterId": "cluster-123", "agentId": "worker" }
}
```

Response:

```json
{ "jsonrpc": "2.0", "id": 9, "result": { "subscriptionId": "sub-logs-1" } }
```

### subscribeClusterTimeline

Request:

```json
{
  "jsonrpc": "2.0",
  "id": 10,
  "method": "subscribeClusterTimeline",
  "params": { "clusterId": "cluster-123" }
}
```

Response:

```json
{ "jsonrpc": "2.0", "id": 10, "result": { "subscriptionId": "sub-timeline-1" } }
```

### unsubscribe

Request:

```json
{
  "jsonrpc": "2.0",
  "id": 11,
  "method": "unsubscribe",
  "params": { "subscriptionId": "sub-logs-1" }
}
```

Response:

```json
{ "jsonrpc": "2.0", "id": 11, "result": { "removed": true } }
```

### getClusterTopology

Request:

```json
{
  "jsonrpc": "2.0",
  "id": 12,
  "method": "getClusterTopology",
  "params": { "clusterId": "cluster-123" }
}
```

Response:

```json
{
  "jsonrpc": "2.0",
  "id": 12,
  "result": {
    "topology": {
      /* ClusterTopology */
    }
  }
}
```

## Notifications

### clusterLogLines

```json
{
  "jsonrpc": "2.0",
  "method": "clusterLogLines",
  "params": {
    "subscriptionId": "sub-logs-1",
    "clusterId": "cluster-123",
    "lines": [
      /* ClusterLogLine[] */
    ],
    "droppedCount": 0
  }
}
```

### clusterTimelineEvents

```json
{
  "jsonrpc": "2.0",
  "method": "clusterTimelineEvents",
  "params": {
    "subscriptionId": "sub-timeline-1",
    "clusterId": "cluster-123",
    "events": [
      /* TimelineEvent[] */
    ],
    "droppedCount": 0
  }
}
```

## Versioning Rules

- `protocolVersion` is required in `initialize` and must match this spec.
- If a client sends an unsupported `protocolVersion`, return error `-32000`.
- Additive changes (new methods/fields) must be backward compatible within the same major protocol version.
- Breaking changes require incrementing `protocolVersion` and updating this spec.
