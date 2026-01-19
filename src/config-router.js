/**
 * Config Router - Maps 2D classification to parameterized templates
 *
 * Single source of truth for: Complexity × TaskType → { base, params }
 * Used by both logic-engine.js (trigger evaluation) and agent-wrapper.js (transform scripts)
 */

const { DEFAULT_MAX_ITERATIONS } = require('./agent/agent-config');

/**
 * Get cluster config based on complexity and task type
 * @param {string} complexity - TRIVIAL, SIMPLE, STANDARD, CRITICAL
 * @param {string} taskType - INQUIRY, TASK, DEBUG
 * @returns {{ base: string, params: object }}
 */
function getConfig(complexity, taskType) {
  const getBase = () => {
    if (taskType === 'DEBUG' && complexity !== 'TRIVIAL') {
      return 'debug-workflow';
    }
    if (complexity === 'TRIVIAL') {
      return 'single-worker';
    }
    if (complexity === 'SIMPLE') {
      return 'worker-validator';
    }
    return 'full-workflow';
  };

  const getLevel = (role) => {
    if (complexity === 'CRITICAL' && role === 'planner') return 'level3';
    if (complexity === 'TRIVIAL') return 'level1';
    return 'level2';
  };

  const getValidatorCount = () => {
    if (complexity === 'TRIVIAL') return 0;
    if (complexity === 'SIMPLE') return 1;
    if (complexity === 'STANDARD') return 2;
    if (complexity === 'CRITICAL') return 4;
    return 1;
  };

  const getMaxTokens = () => {
    if (complexity === 'TRIVIAL') return 50000;
    if (complexity === 'SIMPLE') return 100000;
    if (complexity === 'STANDARD') return 100000;
    if (complexity === 'CRITICAL') return 150000;
    return 100000;
  };

  const base = getBase();

  const params = {
    task_type: taskType,
    complexity,
    max_tokens: getMaxTokens(),
    max_iterations: DEFAULT_MAX_ITERATIONS,
  };

  if (base === 'single-worker') {
    params.worker_level = getLevel('worker');
  } else if (base === 'worker-validator') {
    params.worker_level = getLevel('worker');
    params.validator_level = getLevel('validator');
  } else if (base === 'debug-workflow') {
    params.investigator_level = getLevel('planner');
    params.fixer_level = getLevel('worker');
    params.tester_level = getLevel('validator');
  } else if (base === 'full-workflow') {
    params.planner_level = getLevel('planner');
    params.worker_level = getLevel('worker');
    params.validator_level = getLevel('validator');
    params.validator_count = getValidatorCount();
  }

  return { base, params };
}

module.exports = { getConfig };
