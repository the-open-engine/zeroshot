const Ledger = require('../ledger');
const MessageBus = require('../message-bus');
const LogicEngine = require('../logic-engine');

function maybePublishStageStart({ messageBus, clusterId, logicScript }) {
  const now = Date.now();

  if (logicScript.includes('IMPLEMENTATION_READY')) {
    messageBus.publish({
      cluster_id: clusterId,
      topic: 'IMPLEMENTATION_READY',
      sender: 'worker',
      timestamp: now,
    });
  }

  if (logicScript.includes('QUICK_VALIDATION_PASSED')) {
    messageBus.publish({
      cluster_id: clusterId,
      topic: 'QUICK_VALIDATION_PASSED',
      sender: 'consensus-coordinator',
      timestamp: now,
    });
  }
}

function collectTopicProducers(config) {
  const producersByTopic = new Map();

  for (const agent of config.agents || []) {
    const hooks = agent?.hooks;
    if (!hooks) continue;

    const onComplete = hooks.onComplete;
    if (!onComplete) continue;

    if (onComplete.action === 'publish_message' && onComplete.config?.topic) {
      const topic = String(onComplete.config.topic);
      if (!producersByTopic.has(topic)) {
        producersByTopic.set(topic, new Set());
      }
      producersByTopic.get(topic).add(agent.id);
    }
  }

  return producersByTopic;
}

/**
 * Micro-sim: consensus-like trigger gates must not fire early due to duplicate publishes
 * from the same producer (common in retries / double-publish bugs).
 *
 * Returns an array of error strings.
 */
function simulateConsensusGates(config) {
  const agents = Array.isArray(config.agents) ? config.agents : [];
  const producersByTopic = collectTopicProducers(config);

  const cluster = {
    id: 'template-sim',
    agents: agents.map((a) => ({ id: a.id, role: a.role })),
  };

  const failures = [];

  for (const agent of agents) {
    const isConsensusLike =
      agent?.role === 'coordinator' ||
      String(agent?.id || '').includes('consensus') ||
      String(agent?.id || '').includes('coordinator');

    if (!isConsensusLike) continue;

    for (const trigger of agent.triggers || []) {
      const topic = trigger?.topic;
      const script = trigger?.logic?.script;
      if (!topic || !script) continue;

      const producers = Array.from(producersByTopic.get(topic) || []);
      if (producers.length < 2) continue;

      // Scenario A: Duplicate publishes from one producer MUST NOT satisfy the gate.
      {
        const ledger = new Ledger(':memory:');
        const messageBus = new MessageBus(ledger);
        const logicEngine = new LogicEngine(messageBus, cluster);

        maybePublishStageStart({ messageBus, clusterId: cluster.id, logicScript: script });

        messageBus.publish({
          cluster_id: cluster.id,
          topic,
          sender: producers[0],
          content: { data: { approved: true } },
        });
        messageBus.publish({
          cluster_id: cluster.id,
          topic,
          sender: producers[0],
          content: { data: { approved: true } },
        });

        const shouldTriggerEarly = logicEngine.evaluate(
          script,
          { id: agent.id, cluster_id: cluster.id },
          { topic }
        );
        ledger.close();

        if (shouldTriggerEarly) {
          failures.push(
            `Agent "${agent.id}" trigger on "${topic}" fires early on duplicate sender (${producers[0]}). ` +
              `Gate must require distinct producers: ${producers.join(', ')}`
          );
          continue;
        }
      }

      // Scenario B: One publish from each producer SHOULD satisfy the gate.
      {
        const ledger = new Ledger(':memory:');
        const messageBus = new MessageBus(ledger);
        const logicEngine = new LogicEngine(messageBus, cluster);

        maybePublishStageStart({ messageBus, clusterId: cluster.id, logicScript: script });

        for (const producer of producers) {
          messageBus.publish({
            cluster_id: cluster.id,
            topic,
            sender: producer,
            content: { data: { approved: true } },
          });
        }

        const shouldTrigger = logicEngine.evaluate(
          script,
          { id: agent.id, cluster_id: cluster.id },
          { topic }
        );
        ledger.close();

        if (!shouldTrigger) {
          failures.push(
            `Agent "${agent.id}" trigger on "${topic}" did not fire after all producers published. ` +
              `Expected producers: ${producers.join(', ')}`
          );
        }
      }
    }
  }

  return failures;
}

module.exports = {
  collectTopicProducers,
  simulateConsensusGates,
};
