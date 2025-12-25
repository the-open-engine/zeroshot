/**
 * AgentTriggerEvaluator - Trigger matching and logic evaluation
 *
 * Provides:
 * - Trigger matching based on message topics
 * - Logic evaluation via LogicEngine
 * - Trigger action determination
 */

/**
 * Find trigger matching the message topic
 * @param {Object} params - Evaluation parameters
 * @param {Array} params.triggers - Agent triggers configuration
 * @param {Object} params.message - Message to match against
 * @returns {Object|null} Matching trigger or null
 */
function findMatchingTrigger({ triggers, message }) {
  if (!triggers) {
    return null;
  }

  return triggers.find((trigger) => {
    // Match exact topic or wildcard
    if (trigger.topic === '*' || trigger.topic === message.topic) {
      return true;
    }
    // Match topic prefix (e.g., "VALIDATION_*")
    if (trigger.topic.endsWith('*')) {
      const prefix = trigger.topic.slice(0, -1);
      return message.topic.startsWith(prefix);
    }
    return false;
  });
}

/**
 * Evaluate trigger logic
 * @param {Object} params - Evaluation parameters
 * @param {Object} params.trigger - Trigger to evaluate
 * @param {Object} params.message - Triggering message
 * @param {Object} params.agent - Agent context (id, role, iteration, cluster_id)
 * @param {Object} params.logicEngine - LogicEngine instance
 * @returns {boolean} Whether trigger logic passed
 */
function evaluateTrigger({ trigger, message, agent, logicEngine }) {
  if (!trigger.logic || !trigger.logic.script) {
    return true; // No logic = always true
  }

  // NO TRY/CATCH - let errors propagate and crash
  return logicEngine.evaluate(trigger.logic.script, agent, message);
}

/**
 * Get trigger action type
 * @param {Object} trigger - Trigger object
 * @returns {string} Action type ('execute_task' or 'stop_cluster')
 */
function getTriggerAction(trigger) {
  return trigger.action || 'execute_task';
}

module.exports = {
  findMatchingTrigger,
  evaluateTrigger,
  getTriggerAction,
};
