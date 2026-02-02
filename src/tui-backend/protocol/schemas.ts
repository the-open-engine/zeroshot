const idSchema = {
  anyOf: [{ type: 'string' }, { type: 'number' }],
};

const nullableString = {
  anyOf: [{ type: 'string' }, { type: 'null' }],
};

const nullableNumber = {
  anyOf: [{ type: 'number' }, { type: 'null' }],
};

const errorDataSchema = {
  type: 'object',
  additionalProperties: false,
  properties: {
    detail: { type: 'string' },
    fields: {
      type: 'object',
      additionalProperties: { type: 'string' },
    },
    supportedVersions: { type: 'array', items: { type: 'number' } },
  },
};

const errorSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['code', 'message'],
  properties: {
    code: { type: 'number' },
    message: { type: 'string' },
    data: errorDataSchema,
  },
};

const jsonRpcRequestBase = {
  type: 'object',
  additionalProperties: false,
  required: ['jsonrpc', 'id', 'method'],
  properties: {
    jsonrpc: { const: '2.0' },
    id: idSchema,
    method: { type: 'string' },
    params: { type: ['object', 'array', 'null'] },
  },
};

const jsonRpcNotificationBase = {
  type: 'object',
  additionalProperties: false,
  required: ['jsonrpc', 'method'],
  properties: {
    jsonrpc: { const: '2.0' },
    method: { type: 'string' },
    params: { type: ['object', 'array', 'null'] },
  },
  not: { required: ['id'] },
};

const jsonRpcResponseBase = {
  type: 'object',
  additionalProperties: false,
  required: ['jsonrpc', 'id'],
  properties: {
    jsonrpc: { const: '2.0' },
    id: idSchema,
    // Allow result/error at base layer; method-specific schema handles shape.
    result: {},
    error: {},
  },
};

const emptyParamsSchema = {
  anyOf: [{ type: 'null' }, { type: 'object', additionalProperties: false, maxProperties: 0 }],
};

const clusterSummarySchema = {
  type: 'object',
  additionalProperties: false,
  required: ['id', 'state', 'provider', 'createdAt', 'agentCount', 'messageCount', 'cwd'],
  properties: {
    id: { type: 'string' },
    state: { type: 'string' },
    provider: nullableString,
    createdAt: { type: 'number' },
    agentCount: { type: 'number' },
    messageCount: { type: 'number' },
    cwd: nullableString,
  },
};

const clusterMetricsSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['id', 'supported', 'cpuPercent', 'memoryMB'],
  properties: {
    id: { type: 'string' },
    supported: { type: 'boolean' },
    cpuPercent: nullableNumber,
    memoryMB: nullableNumber,
  },
};

const clusterLogLineSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['id', 'timestamp', 'text', 'agent', 'role', 'sender'],
  properties: {
    id: { type: 'string' },
    timestamp: { type: 'number' },
    text: { type: 'string' },
    agent: nullableString,
    role: nullableString,
    sender: nullableString,
  },
};

const timelineEventSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['id', 'timestamp', 'topic', 'label', 'approved', 'sender'],
  properties: {
    id: { type: 'string' },
    timestamp: { type: 'number' },
    topic: { type: 'string' },
    label: { type: 'string' },
    approved: { anyOf: [{ type: 'boolean' }, { type: 'null' }] },
    sender: nullableString,
  },
};

const topologyAgentSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['id', 'role'],
  properties: {
    id: { type: 'string' },
    role: nullableString,
  },
};

const topologyEdgeSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['from', 'to', 'topic', 'kind'],
  properties: {
    from: { type: 'string' },
    to: { type: 'string' },
    topic: { type: 'string' },
    kind: { enum: ['trigger', 'publish', 'source'] },
    dynamic: { type: 'boolean' },
  },
};

const clusterTopologySchema = {
  type: 'object',
  additionalProperties: false,
  required: ['agents', 'edges', 'topics'],
  properties: {
    agents: { type: 'array', items: topologyAgentSchema },
    edges: { type: 'array', items: topologyEdgeSchema },
    topics: { type: 'array', items: { type: 'string' } },
  },
};

const guidanceDeliveryResultSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['status', 'reason', 'method'],
  properties: {
    status: { type: 'string' },
    reason: nullableString,
    method: nullableString,
    taskId: nullableString,
  },
};

const clusterGuidanceSummarySchema = {
  type: 'object',
  additionalProperties: false,
  required: ['injected', 'queued', 'total'],
  properties: {
    injected: { type: 'number' },
    queued: { type: 'number' },
    total: { type: 'number' },
  },
};

const clusterGuidanceDeliverySchema = {
  type: 'object',
  additionalProperties: false,
  required: ['summary', 'agents', 'timestamp'],
  properties: {
    summary: clusterGuidanceSummarySchema,
    agents: {
      type: 'object',
      additionalProperties: guidanceDeliveryResultSchema,
    },
    timestamp: { type: 'number' },
  },
};

const initializeParamsSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['protocolVersion', 'client'],
  properties: {
    protocolVersion: { type: 'number' },
    client: {
      type: 'object',
      additionalProperties: false,
      required: ['name', 'version'],
      properties: {
        name: { type: 'string' },
        version: { type: 'string' },
        pid: { type: 'number' },
      },
    },
    capabilities: {
      type: 'object',
      additionalProperties: false,
      properties: {
        wantsMetrics: { type: 'boolean' },
        wantsTopology: { type: 'boolean' },
      },
    },
  },
};

const initializeResultSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['protocolVersion', 'server', 'capabilities'],
  properties: {
    protocolVersion: { type: 'number' },
    server: {
      type: 'object',
      additionalProperties: false,
      required: ['name', 'version'],
      properties: {
        name: { type: 'string' },
        version: { type: 'string' },
      },
    },
    capabilities: {
      type: 'object',
      additionalProperties: false,
      required: ['methods', 'notifications'],
      properties: {
        methods: { type: 'array', items: { type: 'string' } },
        notifications: { type: 'array', items: { type: 'string' } },
      },
    },
  },
};

const pingParamsSchema = emptyParamsSchema;

const pingResultSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['ok'],
  properties: {
    ok: { const: true },
  },
};

const listClustersResultSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['clusters'],
  properties: {
    clusters: { type: 'array', items: clusterSummarySchema },
  },
};

const getClusterSummaryParamsSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['clusterId'],
  properties: {
    clusterId: { type: 'string' },
  },
};

const getClusterSummaryResultSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['summary'],
  properties: {
    summary: clusterSummarySchema,
  },
};

const listClusterMetricsParamsSchema = {
  type: 'object',
  additionalProperties: false,
  properties: {
    clusterIds: { type: 'array', items: { type: 'string' } },
  },
};

const listClusterMetricsResultSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['metrics'],
  properties: {
    metrics: { type: 'array', items: clusterMetricsSchema },
  },
};

const startClusterFromTextParamsSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['text'],
  properties: {
    text: { type: 'string' },
    providerOverride: nullableString,
    clusterId: { type: 'string' },
  },
};

const startClusterFromIssueParamsSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['ref'],
  properties: {
    ref: { type: 'string' },
    providerOverride: nullableString,
    clusterId: { type: 'string' },
  },
};

const startClusterResultSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['clusterId'],
  properties: {
    clusterId: { type: 'string' },
  },
};

const sendGuidanceToAgentParamsSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['clusterId', 'agentId', 'text'],
  properties: {
    clusterId: { type: 'string' },
    agentId: { type: 'string' },
    text: { type: 'string' },
    timeoutMs: { type: 'number' },
  },
};

const sendGuidanceToClusterParamsSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['clusterId', 'text'],
  properties: {
    clusterId: { type: 'string' },
    text: { type: 'string' },
    timeoutMs: { type: 'number' },
  },
};

const sendGuidanceToAgentResultSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['result'],
  properties: {
    result: guidanceDeliveryResultSchema,
  },
};

const sendGuidanceToClusterResultSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['result'],
  properties: {
    result: clusterGuidanceDeliverySchema,
  },
};

const subscribeClusterLogsParamsSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['clusterId'],
  properties: {
    clusterId: { type: 'string' },
    agentId: nullableString,
  },
};

const subscribeClusterTimelineParamsSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['clusterId'],
  properties: {
    clusterId: { type: 'string' },
  },
};

const subscribeResultSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['subscriptionId'],
  properties: {
    subscriptionId: { type: 'string' },
  },
};

const unsubscribeParamsSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['subscriptionId'],
  properties: {
    subscriptionId: { type: 'string' },
  },
};

const unsubscribeResultSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['removed'],
  properties: {
    removed: { type: 'boolean' },
  },
};

const getClusterTopologyParamsSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['clusterId'],
  properties: {
    clusterId: { type: 'string' },
  },
};

const getClusterTopologyResultSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['topology'],
  properties: {
    topology: clusterTopologySchema,
  },
};

const clusterLogLinesNotificationSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['subscriptionId', 'clusterId', 'lines'],
  properties: {
    subscriptionId: { type: 'string' },
    clusterId: { type: 'string' },
    lines: { type: 'array', items: clusterLogLineSchema },
    droppedCount: { type: 'number' },
  },
};

const clusterTimelineEventsNotificationSchema = {
  type: 'object',
  additionalProperties: false,
  required: ['subscriptionId', 'clusterId', 'events'],
  properties: {
    subscriptionId: { type: 'string' },
    clusterId: { type: 'string' },
    events: { type: 'array', items: timelineEventSchema },
    droppedCount: { type: 'number' },
  },
};

const buildRequestSchema = (method, paramsSchema, paramsRequired) => {
  const schema = {
    ...jsonRpcRequestBase,
    properties: {
      ...jsonRpcRequestBase.properties,
      method: { const: method },
    },
  };
  if (paramsSchema) {
    schema.properties.params = paramsSchema;
  }
  if (paramsRequired) {
    schema.required = [...jsonRpcRequestBase.required, 'params'];
  }
  return schema;
};

const buildResponseSchema = (resultSchema) => ({
  ...jsonRpcResponseBase,
  required: [...jsonRpcResponseBase.required, 'result'],
  properties: {
    ...jsonRpcResponseBase.properties,
    result: resultSchema,
  },
});

const buildErrorResponseSchema = () => ({
  ...jsonRpcResponseBase,
  required: [...jsonRpcResponseBase.required, 'error'],
  properties: {
    ...jsonRpcResponseBase.properties,
    error: errorSchema,
  },
});

const buildNotificationSchema = (method, paramsSchema) => ({
  ...jsonRpcNotificationBase,
  properties: {
    ...jsonRpcNotificationBase.properties,
    method: { const: method },
    params: paramsSchema,
  },
  required: [...jsonRpcNotificationBase.required, 'params'],
});

const REQUEST_SCHEMAS = {
  initialize: buildRequestSchema('initialize', initializeParamsSchema, true),
  ping: buildRequestSchema('ping', pingParamsSchema, true),
  listClusters: buildRequestSchema('listClusters', emptyParamsSchema, false),
  getClusterSummary: buildRequestSchema('getClusterSummary', getClusterSummaryParamsSchema, true),
  listClusterMetrics: buildRequestSchema(
    'listClusterMetrics',
    listClusterMetricsParamsSchema,
    false
  ),
  startClusterFromText: buildRequestSchema(
    'startClusterFromText',
    startClusterFromTextParamsSchema,
    true
  ),
  startClusterFromIssue: buildRequestSchema(
    'startClusterFromIssue',
    startClusterFromIssueParamsSchema,
    true
  ),
  sendGuidanceToAgent: buildRequestSchema(
    'sendGuidanceToAgent',
    sendGuidanceToAgentParamsSchema,
    true
  ),
  sendGuidanceToCluster: buildRequestSchema(
    'sendGuidanceToCluster',
    sendGuidanceToClusterParamsSchema,
    true
  ),
  subscribeClusterLogs: buildRequestSchema(
    'subscribeClusterLogs',
    subscribeClusterLogsParamsSchema,
    true
  ),
  subscribeClusterTimeline: buildRequestSchema(
    'subscribeClusterTimeline',
    subscribeClusterTimelineParamsSchema,
    true
  ),
  unsubscribe: buildRequestSchema('unsubscribe', unsubscribeParamsSchema, true),
  getClusterTopology: buildRequestSchema(
    'getClusterTopology',
    getClusterTopologyParamsSchema,
    true
  ),
};

const RESPONSE_SCHEMAS = {
  initialize: buildResponseSchema(initializeResultSchema),
  ping: buildResponseSchema(pingResultSchema),
  listClusters: buildResponseSchema(listClustersResultSchema),
  getClusterSummary: buildResponseSchema(getClusterSummaryResultSchema),
  listClusterMetrics: buildResponseSchema(listClusterMetricsResultSchema),
  startClusterFromText: buildResponseSchema(startClusterResultSchema),
  startClusterFromIssue: buildResponseSchema(startClusterResultSchema),
  sendGuidanceToAgent: buildResponseSchema(sendGuidanceToAgentResultSchema),
  sendGuidanceToCluster: buildResponseSchema(sendGuidanceToClusterResultSchema),
  subscribeClusterLogs: buildResponseSchema(subscribeResultSchema),
  subscribeClusterTimeline: buildResponseSchema(subscribeResultSchema),
  unsubscribe: buildResponseSchema(unsubscribeResultSchema),
  getClusterTopology: buildResponseSchema(getClusterTopologyResultSchema),
};

const NOTIFICATION_SCHEMAS = {
  clusterLogLines: buildNotificationSchema('clusterLogLines', clusterLogLinesNotificationSchema),
  clusterTimelineEvents: buildNotificationSchema(
    'clusterTimelineEvents',
    clusterTimelineEventsNotificationSchema
  ),
};

module.exports = {
  errorSchema,
  jsonRpcRequestBase,
  jsonRpcNotificationBase,
  jsonRpcResponseBase,
  buildErrorResponseSchema,
  REQUEST_SCHEMAS,
  RESPONSE_SCHEMAS,
  NOTIFICATION_SCHEMAS,
};
