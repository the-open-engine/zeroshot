const Ledger = require('../ledger');
const MessageBus = require('../message-bus');
const LogicEngine = require('../logic-engine');

const STAGE_START_TOPICS = ['IMPLEMENTATION_READY', 'QUICK_VALIDATION_PASSED'];
const EXTERNAL_STAGE_SENDERS = {
  IMPLEMENTATION_READY: 'worker',
  QUICK_VALIDATION_PASSED: 'consensus-coordinator',
};

function scriptReferencesTopic(logicScript, topic) {
  return logicScript.includes(`topic: '${topic}'`) || logicScript.includes(`topic: "${topic}"`);
}

function getRequiredStageTopics(logicScript) {
  return STAGE_START_TOPICS.filter((topic) => scriptReferencesTopic(logicScript, topic));
}

function publishStageStartMessages({
  messageBus,
  clusterId,
  producersByTopic,
  requiredStageTopics,
  allowExternalTopics,
}) {
  let timestamp = Date.now();

  for (const topic of requiredStageTopics) {
    const producers = Array.from(producersByTopic.get(topic) || []);
    const sender =
      producers[0] || (allowExternalTopics.includes(topic) ? EXTERNAL_STAGE_SENDERS[topic] : null);
    if (!sender) {
      continue;
    }

    messageBus.publish({
      cluster_id: clusterId,
      topic,
      sender,
      timestamp: timestamp++,
    });
  }
}

function collectTopicProducers(config) {
  const producersByTopic = new Map();

  for (const agent of config.agents || []) {
    const topic = getPublishedTopic(agent);
    if (!topic) continue;
    if (!producersByTopic.has(topic)) {
      producersByTopic.set(topic, new Set());
    }
    producersByTopic.get(topic).add(agent.id);
  }

  return producersByTopic;
}

function getPublishedTopic(agent) {
  const onComplete = agent?.hooks?.onComplete;
  if (!onComplete) return null;
  if (onComplete.action !== 'publish_message') return null;
  if (!onComplete.config?.topic) return null;
  return String(onComplete.config.topic);
}

function hasConsensusLikeId(agentId) {
  return agentId.includes('consensus') || agentId.includes('coordinator');
}

function hasStopClusterTrigger(agent) {
  return agent?.triggers?.some((trigger) => trigger.action === 'stop_cluster');
}

function isConsensusLikeAgent(agent) {
  const agentId = String(agent?.id || '');
  const explicitIds = ['git-pusher', 'completion-detector'];

  return (
    agent?.role === 'coordinator' ||
    explicitIds.includes(agentId) ||
    hasConsensusLikeId(agentId) ||
    hasStopClusterTrigger(agent)
  );
}

function getMissingStageTopics(requiredStageTopics, producersByTopic, allowExternalTopics) {
  return requiredStageTopics.filter((stageTopic) => {
    const producers = producersByTopic.get(stageTopic);
    return (!producers || producers.size === 0) && !allowExternalTopics.includes(stageTopic);
  });
}

function createSimulationContext(config, options = {}) {
  const agents = Array.isArray(config.agents) ? config.agents : [];

  return {
    agents,
    producersByTopic: collectTopicProducers(config),
    allowExternalTopics: Array.isArray(options.allowExternalTopics)
      ? options.allowExternalTopics
      : [],
    cluster: {
      id: 'template-sim',
      agents: agents.map((a) => ({
        ...a,
        // Mirror runtime AgentWrapper shape so trigger scripts can inspect either
        // `candidate.hooks` (resolved config) or `candidate.config.hooks` (live cluster).
        config: a,
      })),
    },
  };
}

function evaluateScenario({
  agentId,
  cluster,
  topic,
  script,
  producers,
  producersByTopic,
  requiredStageTopics,
  allowExternalTopics,
  publishMessages,
}) {
  const ledger = new Ledger(':memory:');
  const messageBus = new MessageBus(ledger);
  const logicEngine = new LogicEngine(messageBus, cluster);

  try {
    publishStageStartMessages({
      messageBus,
      clusterId: cluster.id,
      producersByTopic,
      requiredStageTopics,
      allowExternalTopics,
    });

    publishMessages(messageBus, producers, cluster.id);

    return logicEngine.evaluate(
      script,
      { id: agentId, cluster_id: cluster.id },
      { topic, cluster_id: cluster.id }
    );
  } finally {
    ledger.close();
  }
}

function checkDuplicateProducerScenario(context) {
  return evaluateScenario({
    ...context,
    publishMessages(messageBus, producers, clusterId) {
      messageBus.publish({
        cluster_id: clusterId,
        topic: context.topic,
        sender: producers[0],
        content: { data: { approved: true } },
      });
      messageBus.publish({
        cluster_id: clusterId,
        topic: context.topic,
        sender: producers[0],
        content: { data: { approved: true } },
      });
    },
  });
}

function checkDistinctProducerScenario(context) {
  return evaluateScenario({
    ...context,
    publishMessages(messageBus, producers, clusterId) {
      for (const producer of producers) {
        messageBus.publish({
          cluster_id: clusterId,
          topic: context.topic,
          sender: producer,
          content: { data: { approved: true } },
        });
      }
    },
  });
}

function getMissingStageFailure(agentId, topic, missingStageTopics) {
  if (missingStageTopics.length === 0) {
    return null;
  }

  return (
    `Agent "${agentId}" trigger on "${topic}" depends on missing stage topic(s): ${missingStageTopics.join(', ')}. ` +
    'Preflight must validate real producers, not synthesize stage-start messages.'
  );
}

function getConsensusScenarioContext(agent, trigger, simulation) {
  const topic = trigger?.topic;
  const script = trigger?.logic?.script;
  if (!topic || !script) {
    return null;
  }

  const requiredStageTopics = getRequiredStageTopics(script);
  const missingStageTopics = getMissingStageTopics(
    requiredStageTopics,
    simulation.producersByTopic,
    simulation.allowExternalTopics
  );
  const missingStageFailure = getMissingStageFailure(agent.id, topic, missingStageTopics);
  if (missingStageFailure) {
    return { failure: missingStageFailure };
  }

  const producers = Array.from(simulation.producersByTopic.get(topic) || []);
  if (producers.length < 2) {
    return { failure: null };
  }

  return {
    context: {
      agentId: agent.id,
      cluster: simulation.cluster,
      topic,
      script,
      producers,
      producersByTopic: simulation.producersByTopic,
      requiredStageTopics,
      allowExternalTopics: simulation.allowExternalTopics,
    },
    failure: null,
  };
}

function validateConsensusTrigger(agent, trigger, simulation) {
  const scenario = getConsensusScenarioContext(agent, trigger, simulation);
  if (!scenario) {
    return [];
  }

  if (scenario.failure) {
    return [scenario.failure];
  }

  if (!scenario.context) {
    return [];
  }

  const { context } = scenario;
  const { producers, topic } = context;

  const failures = [];
  if (checkDuplicateProducerScenario(context)) {
    failures.push(
      `Agent "${agent.id}" trigger on "${topic}" fires early on duplicate sender (${producers[0]}). ` +
        `Gate must require distinct producers: ${producers.join(', ')}`
    );
  }

  if (!checkDistinctProducerScenario(context)) {
    failures.push(
      `Agent "${agent.id}" trigger on "${topic}" did not fire after all producers published. ` +
        `Expected producers: ${producers.join(', ')}`
    );
  }

  return failures;
}

/**
 * Micro-sim: consensus-like trigger gates must not fire early due to duplicate publishes
 * from the same producer (common in retries / double-publish bugs).
 *
 * Returns an array of error strings.
 */
function simulateConsensusGates(config, options = {}) {
  const simulation = createSimulationContext(config, options);
  const failures = [];

  for (const agent of simulation.agents) {
    if (!isConsensusLikeAgent(agent)) continue;
    for (const trigger of agent.triggers || []) {
      failures.push(...validateConsensusTrigger(agent, trigger, simulation));
    }
  }

  return failures;
}

module.exports = {
  collectTopicProducers,
  simulateConsensusGates,
};
