const MOCK_LAUNCH_ENV = 'ZEROSHOT_TUI_BACKEND_MOCK_LAUNCH';
const MOCK_GUIDANCE_ENV = 'ZEROSHOT_TUI_BACKEND_MOCK_GUIDANCE';
const METRICS_PLATFORM_ENV = 'ZEROSHOT_TUI_BACKEND_METRICS_PLATFORM';

const path = require('path');
const {
  createValidator,
  createDispatcher,
  createFrameParser,
  encodeFrame,
  RPC_ERROR_CODES,
  RPC_ERROR_MESSAGES,
  PROTOCOL_VERSION,
} = require('./protocol');
const {
  listClusters,
  getClusterSummary,
  listClusterMetrics,
  ClusterNotFoundError,
} = require('./services/cluster-registry');
const { getClusterTopology } = require('./services/cluster-topology');
const {
  launchClusterFromText,
  launchClusterFromIssue,
  InvalidIssueReferenceError,
} = require('./services/cluster-launcher');
const { createClusterLogStream, MAX_LOG_LINES } = require('./services/cluster-logs');
const { createClusterTimelineStream, MAX_TIMELINE_EVENTS } = require('./services/cluster-timeline');
const { sendAgentGuidance, sendClusterGuidance } = require('./services/guidance-delivery');
const { createSubscriptionRegistry } = require('./subscriptions');

const isValidId = (value) => typeof value === 'string' || typeof value === 'number';

const isMockLaunchEnabled = () => process.env[MOCK_LAUNCH_ENV] === '1';
const isMockGuidanceEnabled = () => process.env[MOCK_GUIDANCE_ENV] === '1';

const createMockLauncherDeps = () => ({
  getOrchestrator: async () => ({}),
  loadSettings: () => ({ defaultConfig: 'conductor-bootstrap', providerSettings: {} }),
  resolveConfigPath: () => 'mock-config',
  loadClusterConfig: () => ({}),
  startClusterFromText: async () => {},
  startClusterFromIssue: async () => {},
});

const createMockGuidanceDeps = () => ({
  getOrchestrator: async () => ({
    sendGuidanceToAgent: async (clusterId, agentId) => ({
      status: 'injected',
      reason: null,
      method: 'pty',
      taskId: `task-${agentId}`,
    }),
    sendGuidanceToCluster: async () => ({
      summary: { injected: 1, queued: 1, total: 2 },
      agents: {
        'mock-agent-1': {
          status: 'injected',
          reason: null,
          method: 'pty',
          taskId: 'task-mock-agent-1',
        },
        'mock-agent-2': {
          status: 'queued',
          reason: 'queued',
          method: null,
          taskId: 'task-mock-agent-2',
        },
      },
      timestamp: 1700000000000,
    }),
  }),
});

const loadPackageInfo = () => {
  try {
    const packagePath = path.resolve(__dirname, '..', '..', 'package.json');
    const pkg = require(packagePath);
    return {
      name: typeof pkg.name === 'string' ? pkg.name : 'zeroshot',
      version: typeof pkg.version === 'string' ? pkg.version : '0.0.0',
    };
  } catch (error) {
    return { name: 'zeroshot', version: '0.0.0' };
  }
};

const writeFrame = (payload) => {
  const framed = encodeFrame(payload);
  process.stdout.write(framed);
};

const writeError = (id, error) => {
  writeFrame({
    jsonrpc: '2.0',
    id,
    error,
  });
};

const buildRpcError = (code, message, detail) =>
  detail ? { code, message, data: { detail } } : { code, message };

const logDiagnostic = (message, error) => {
  const details = error instanceof Error ? `${message}: ${error.stack || error.message}` : message;
  process.stderr.write(`${details}\n`);
};

const isNonEmptyString = (value) => typeof value === 'string' && value.trim().length > 0;

const resolveMetricsPlatformOverride = () => {
  const value = process.env[METRICS_PLATFORM_ENV];
  return isNonEmptyString(value) ? value : null;
};

const isRpcError = (error) =>
  error &&
  typeof error === 'object' &&
  typeof error.code === 'number' &&
  typeof error.message === 'string';

const isGuidanceInvalidParamsError = (message) =>
  message.includes('is required') ||
  message.includes('non-empty string') ||
  message.includes('agent not found');

const isGuidanceClusterNotFoundError = (message) => message.includes('cluster not found');

const isTopologyClusterNotFoundError = (error) =>
  error instanceof Error && /cluster/i.test(error.message) && /not found/i.test(error.message);

const validateGuidanceText = (text) => {
  if (!isNonEmptyString(text)) {
    throw buildRpcError(
      RPC_ERROR_CODES.INVALID_PARAMS,
      RPC_ERROR_MESSAGES[RPC_ERROR_CODES.INVALID_PARAMS],
      'text must be a non-empty string'
    );
  }
};

const validateGuidanceId = (value, label) => {
  if (!isNonEmptyString(value)) {
    throw buildRpcError(
      RPC_ERROR_CODES.INVALID_PARAMS,
      RPC_ERROR_MESSAGES[RPC_ERROR_CODES.INVALID_PARAMS],
      `${label} must be a non-empty string`
    );
  }
};

const resolveGuidanceError = (error) => {
  if (isRpcError(error)) {
    return error;
  }
  const message = error instanceof Error ? error.message : 'Guidance delivery error';
  if (isGuidanceClusterNotFoundError(message)) {
    return buildRpcError(
      RPC_ERROR_CODES.CLUSTER_NOT_FOUND,
      RPC_ERROR_MESSAGES[RPC_ERROR_CODES.CLUSTER_NOT_FOUND],
      message
    );
  }
  if (isGuidanceInvalidParamsError(message)) {
    return buildRpcError(
      RPC_ERROR_CODES.INVALID_PARAMS,
      RPC_ERROR_MESSAGES[RPC_ERROR_CODES.INVALID_PARAMS],
      message
    );
  }
  return buildRpcError(
    RPC_ERROR_CODES.INTERNAL_ERROR,
    RPC_ERROR_MESSAGES[RPC_ERROR_CODES.INTERNAL_ERROR],
    message
  );
};

const capPayload = (items, maxItems) => {
  if (!Array.isArray(items)) {
    return { items: [], droppedCount: 0 };
  }
  if (items.length <= maxItems) {
    return { items, droppedCount: 0 };
  }
  const trimmed = items.slice(items.length - maxItems);
  return { items: trimmed, droppedCount: items.length - trimmed.length };
};

const startServer = () => {
  const registry = createSubscriptionRegistry();
  const notifications = ['clusterLogLines', 'clusterTimelineEvents'];
  let shuttingDown = false;
  const validator = createValidator();
  const dispatcher = createDispatcher({
    serverInfo: loadPackageInfo(),
    protocolVersion: PROTOCOL_VERSION,
    notifications,
    handlers: {
      listClusters: async () => ({
        clusters: await listClusters(),
      }),
      getClusterSummary: async (params) => {
        try {
          const summary = await getClusterSummary({
            clusterId: params.clusterId,
          });
          return { summary };
        } catch (error) {
          if (error instanceof ClusterNotFoundError) {
            throw buildRpcError(
              RPC_ERROR_CODES.CLUSTER_NOT_FOUND,
              RPC_ERROR_MESSAGES[RPC_ERROR_CODES.CLUSTER_NOT_FOUND],
              error.message
            );
          }
          throw error;
        }
      },
      getClusterTopology: async (params) => {
        try {
          const topology = await getClusterTopology(params.clusterId);
          return { topology };
        } catch (error) {
          if (isTopologyClusterNotFoundError(error)) {
            throw buildRpcError(
              RPC_ERROR_CODES.CLUSTER_NOT_FOUND,
              RPC_ERROR_MESSAGES[RPC_ERROR_CODES.CLUSTER_NOT_FOUND],
              error instanceof Error ? error.message : 'Cluster not found'
            );
          }
          throw error;
        }
      },
      listClusterMetrics: async (params) => {
        const clusterIds = Array.isArray(params?.clusterIds) ? params.clusterIds : undefined;
        const platformOverride = resolveMetricsPlatformOverride();
        const metricsById = await listClusterMetrics({
          clusterIds,
          deps: platformOverride ? { platform: platformOverride } : undefined,
        });
        const metrics = Array.isArray(clusterIds)
          ? clusterIds.map((id) => metricsById[id]).filter(Boolean)
          : Object.values(metricsById);
        return { metrics };
      },
      startClusterFromText: async (params) => {
        try {
          const result = await launchClusterFromText({
            text: params.text,
            providerOverride: params.providerOverride ?? null,
            clusterId: params.clusterId,
            deps: isMockLaunchEnabled() ? createMockLauncherDeps() : undefined,
          });
          return result;
        } catch (error) {
          throw buildRpcError(
            RPC_ERROR_CODES.INTERNAL_ERROR,
            RPC_ERROR_MESSAGES[RPC_ERROR_CODES.INTERNAL_ERROR],
            error instanceof Error ? error.message : 'Launcher error'
          );
        }
      },
      startClusterFromIssue: async (params) => {
        try {
          const result = await launchClusterFromIssue({
            ref: params.ref,
            providerOverride: params.providerOverride ?? null,
            clusterId: params.clusterId,
            deps: isMockLaunchEnabled() ? createMockLauncherDeps() : undefined,
          });
          return result;
        } catch (error) {
          if (error instanceof InvalidIssueReferenceError) {
            throw buildRpcError(
              RPC_ERROR_CODES.INVALID_PARAMS,
              RPC_ERROR_MESSAGES[RPC_ERROR_CODES.INVALID_PARAMS],
              error.message
            );
          }
          throw buildRpcError(
            RPC_ERROR_CODES.INTERNAL_ERROR,
            RPC_ERROR_MESSAGES[RPC_ERROR_CODES.INTERNAL_ERROR],
            error instanceof Error ? error.message : 'Launcher error'
          );
        }
      },
      sendGuidanceToAgent: async (params) => {
        try {
          validateGuidanceId(params?.clusterId, 'clusterId');
          validateGuidanceId(params?.agentId, 'agentId');
          validateGuidanceText(params?.text);
          const result = await sendAgentGuidance({
            clusterId: params.clusterId,
            agentId: params.agentId,
            text: params.text,
            timeoutMs: params.timeoutMs,
            deps: isMockGuidanceEnabled() ? createMockGuidanceDeps() : undefined,
          });
          return { result };
        } catch (error) {
          throw resolveGuidanceError(error);
        }
      },
      sendGuidanceToCluster: async (params) => {
        try {
          validateGuidanceId(params?.clusterId, 'clusterId');
          validateGuidanceText(params?.text);
          const result = await sendClusterGuidance({
            clusterId: params.clusterId,
            text: params.text,
            timeoutMs: params.timeoutMs,
            deps: isMockGuidanceEnabled() ? createMockGuidanceDeps() : undefined,
          });
          return { result };
        } catch (error) {
          throw resolveGuidanceError(error);
        }
      },
      subscribeClusterLogs: async (params) => {
        const clusterId = params.clusterId;
        const agentId = params.agentId ?? null;
        let subscriptionId = '';
        const stream = createClusterLogStream({
          clusterId,
          agentId,
          maxInitialLines: MAX_LOG_LINES * 5,
          onLines: (lines) => {
            if (!subscriptionId) return;
            const { items, droppedCount } = capPayload(lines, MAX_LOG_LINES);
            if (!items.length) return;
            const payload =
              droppedCount > 0
                ? {
                    subscriptionId,
                    clusterId,
                    lines: items,
                    droppedCount,
                  }
                : {
                    subscriptionId,
                    clusterId,
                    lines: items,
                  };
            writeFrame({
              jsonrpc: '2.0',
              method: 'clusterLogLines',
              params: payload,
            });
          },
        });
        subscriptionId = registry.add('clusterLogs', () => stream.close());
        stream.start();
        return { subscriptionId };
      },
      subscribeClusterTimeline: async (params) => {
        const clusterId = params.clusterId;
        let subscriptionId = '';
        const stream = createClusterTimelineStream({
          clusterId,
          maxInitialEvents: MAX_TIMELINE_EVENTS * 5,
          onEvents: (events) => {
            if (!subscriptionId) return;
            const { items, droppedCount } = capPayload(events, MAX_TIMELINE_EVENTS);
            if (!items.length) return;
            const payload =
              droppedCount > 0
                ? {
                    subscriptionId,
                    clusterId,
                    events: items,
                    droppedCount,
                  }
                : {
                    subscriptionId,
                    clusterId,
                    events: items,
                  };
            writeFrame({
              jsonrpc: '2.0',
              method: 'clusterTimelineEvents',
              params: payload,
            });
          },
        });
        subscriptionId = registry.add('clusterTimeline', () => stream.close());
        stream.start();
        return { subscriptionId };
      },
      unsubscribe: async (params) => registry.unsubscribe(params.subscriptionId),
    },
  });
  const parser = createFrameParser();

  const shutdown = (code) => {
    if (shuttingDown) return;
    shuttingDown = true;
    registry.closeAll();
    process.exit(code);
  };

  const handleFrame = async (payload) => {
    let message;
    try {
      message = JSON.parse(payload);
    } catch (error) {
      writeError(null, {
        code: RPC_ERROR_CODES.PARSE_ERROR,
        message: RPC_ERROR_MESSAGES[RPC_ERROR_CODES.PARSE_ERROR],
      });
      logDiagnostic('Invalid JSON payload', error);
      return;
    }

    if (!message || typeof message !== 'object') {
      writeError(null, {
        code: RPC_ERROR_CODES.INVALID_REQUEST,
        message: RPC_ERROR_MESSAGES[RPC_ERROR_CODES.INVALID_REQUEST],
      });
      return;
    }

    const hasId = Object.prototype.hasOwnProperty.call(message, 'id');
    if (!hasId) {
      const notification = validator.validateNotification(message);
      if (!notification.ok) {
        logDiagnostic('Invalid notification received', notification.error);
      }
      return;
    }

    const requestValidation = validator.validateRequest(message);
    if (!requestValidation.ok) {
      const responseId = isValidId(message.id) ? message.id : null;
      writeError(responseId, requestValidation.error);
      return;
    }

    const dispatchResult = await dispatcher.dispatchRequest(requestValidation.value);
    if (!dispatchResult.ok) {
      writeError(message.id, dispatchResult.error);
      return;
    }

    writeFrame({
      jsonrpc: '2.0',
      id: message.id,
      result: dispatchResult.result,
    });
  };

  const handleChunk = (chunk) => {
    let frames = [];
    try {
      frames = parser.push(chunk);
    } catch (error) {
      parser.reset();
      writeError(null, {
        code: RPC_ERROR_CODES.PARSE_ERROR,
        message: RPC_ERROR_MESSAGES[RPC_ERROR_CODES.PARSE_ERROR],
        data: { detail: error instanceof Error ? error.message : 'Parse error' },
      });
      logDiagnostic('Frame parsing failed', error);
      return;
    }

    for (const frame of frames) {
      void handleFrame(frame);
    }
  };

  process.stdin.on('data', handleChunk);
  process.stdin.on('end', () => {
    shutdown(0);
  });
  process.stdin.on('error', (error) => {
    logDiagnostic('Stdin error', error);
    shutdown(1);
  });

  process.on('uncaughtException', (error) => {
    logDiagnostic('Uncaught exception', error);
    shutdown(1);
  });
  process.on('unhandledRejection', (error) => {
    logDiagnostic('Unhandled rejection', error);
    shutdown(1);
  });
  process.on('exit', () => {
    if (!shuttingDown) {
      shuttingDown = true;
      registry.closeAll();
    }
  });

  process.stdin.resume();
};

if (require.main === module) {
  startServer();
}

module.exports = {
  startServer,
};
