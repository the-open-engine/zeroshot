/**
 * Hook Sandbox - Shared VM sandbox builder for transforms and logic scripts
 */

function buildLedgerAPI(messageBus, clusterId) {
  if (!messageBus) return null;
  return {
    query: (criteria) => messageBus.query({ ...criteria, cluster_id: clusterId }),
    findLast: (criteria) => messageBus.findLast({ ...criteria, cluster_id: clusterId }),
    count: (criteria) => messageBus.count({ ...criteria, cluster_id: clusterId }),
    since: (timestamp) => messageBus.since({ cluster_id: clusterId, timestamp }),
  };
}

function buildClusterAPI(clusterId, cluster) {
  return {
    id: clusterId,
    getAgents: () => (cluster ? cluster.agents || [] : []),
    getAgentsByRole: (role) =>
      cluster ? (cluster.agents || []).filter((a) => a.role === role) : [],
    getAgent: (id) => (cluster ? (cluster.agents || []).find((a) => a.id === id) : null),
  };
}

function buildHelpers(ledgerAPI) {
  return {
    getConfig: require('../config-router').getConfig,
    allResponded: (agents, topic, since) => {
      if (!ledgerAPI) return false;
      const responses = ledgerAPI.query({ topic, since });
      const responders = new Set(responses.map((r) => r.sender));
      return agents.every((a) => responders.has(a.id || a));
    },
    hasConsensus: (topic, since) => {
      if (!ledgerAPI) return false;
      const responses = ledgerAPI.query({ topic, since });
      if (responses.length === 0) return false;
      return responses.every((r) => r.content?.data?.approved === true);
    },
  };
}

function buildSandbox({ agent, context, resultData, logPrefix }) {
  const clusterId = agent.cluster?.id || context.cluster?.id || agent.cluster_id || 'unknown';
  const messageBus = agent.messageBus;
  const cluster = context.cluster || agent.cluster || null;

  const ledgerAPI = buildLedgerAPI(messageBus, clusterId);
  const clusterAPI = buildClusterAPI(clusterId, cluster);
  const helpers = buildHelpers(ledgerAPI);

  return {
    result: resultData || {},
    triggeringMessage: context.triggeringMessage || null,
    ledger: ledgerAPI,
    cluster: clusterAPI,
    helpers,
    Set,
    Map,
    Array,
    Object,
    String,
    Number,
    Boolean,
    Math,
    Date,
    JSON,
    console: {
      log: (...args) => agent._log(logPrefix, ...args),
      error: (...args) => console.error(logPrefix, ...args),
      warn: (...args) => console.warn(logPrefix, ...args),
    },
  };
}

module.exports = { buildSandbox };
