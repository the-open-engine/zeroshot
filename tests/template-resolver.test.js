/**
 * Tests for TemplateResolver and 2D classification routing
 */

const assert = require('assert');
const path = require('path');
const TemplateResolver = require('../src/template-resolver');
const { DEFAULT_MAX_ITERATIONS } = require('../src/agent/agent-config');

// Copy of getConfig logic from config-router.js for testing
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

const COMPLEXITIES = ['TRIVIAL', 'SIMPLE', 'STANDARD', 'CRITICAL'];
const TASK_TYPES = ['INQUIRY', 'TASK', 'DEBUG'];

describe('TemplateResolver', function () {
  let resolver;

  before(function () {
    const templatesDir = path.join(__dirname, '..', 'cluster-templates');
    resolver = new TemplateResolver(templatesDir);
  });

  describe('listTemplates', function () {
    it('should list all base templates', function () {
      const templates = resolver.listTemplates();
      assert.ok(templates.includes('single-worker'));
      assert.ok(templates.includes('worker-validator'));
      assert.ok(templates.includes('debug-workflow'));
      assert.ok(templates.includes('full-workflow'));
    });
  });

  describe('getTemplateInfo', function () {
    it('should return template metadata', function () {
      const info = resolver.getTemplateInfo('single-worker');
      assert.ok(info.name);
      assert.ok(info.description);
      assert.ok(info.params);
      assert.ok(info.params.worker_level);
    });

    it('should return null for non-existent template', function () {
      const info = resolver.getTemplateInfo('does-not-exist');
      assert.strictEqual(info, null);
    });
  });

  describe('resolve', function () {
    it('should resolve single-worker template', function () {
      const resolved = resolver.resolve('single-worker', {
        task_type: 'TASK',
        complexity: 'TRIVIAL',
        max_tokens: 50000,
        max_iterations: DEFAULT_MAX_ITERATIONS,
        worker_level: 'level1',
      });

      assert.ok(resolved.agents);
      assert.strictEqual(resolved.agents.length, 1);

      const worker = resolved.agents.find((a) => a.id === 'worker');
      assert.strictEqual(worker.modelLevel, 'level1');
    });

    it('should resolve full-workflow with conditional validators', function () {
      const resolved = resolver.resolve('full-workflow', {
        task_type: 'TASK',
        complexity: 'CRITICAL',
        max_tokens: 150000,
        max_iterations: DEFAULT_MAX_ITERATIONS,
        planner_level: 'level3',
        worker_level: 'level2',
        validator_level: 'level2',
        validator_count: 4,
      });

      assert.ok(resolved.agents);

      const planner = resolved.agents.find((a) => a.id === 'planner');
      assert.strictEqual(planner.modelLevel, 'level3');

      // Should have 5 validators for CRITICAL
      const validators = resolved.agents.filter((a) => a.role === 'validator');
      assert.strictEqual(validators.length, 5);
    });

    it('should fail on missing required params', function () {
      assert.throws(() => resolver.resolve('single-worker', {}), /Missing required params/);
    });

    it('should fail on non-existent template', function () {
      assert.throws(() => resolver.resolve('does-not-exist', {}), /Base template not found/);
    });
  });
});

describe('2D Classification Routing', function () {
  let resolver;

  before(function () {
    const templatesDir = path.join(__dirname, '..', 'cluster-templates');
    resolver = new TemplateResolver(templatesDir);
  });

  describe('All 12 combinations', function () {
    for (const complexity of COMPLEXITIES) {
      for (const taskType of TASK_TYPES) {
        const key = `${complexity}:${taskType}`;

        it(`should resolve ${key}`, function () {
          const { base, params } = getConfig(complexity, taskType);
          const resolved = resolver.resolve(base, params);

          assert.ok(resolved.agents, `${key}: No agents`);
          assert.ok(resolved.agents.length > 0, `${key}: Empty agents array`);
        });
      }
    }
  });

  describe('Classification correctness', function () {
    it('TRIVIAL should use single-worker with level1', function () {
      const { base, params } = getConfig('TRIVIAL', 'TASK');
      assert.strictEqual(base, 'single-worker');
      assert.strictEqual(params.worker_level, 'level1');
    });

    it('SIMPLE DEBUG should use debug-workflow', function () {
      const { base, params } = getConfig('SIMPLE', 'DEBUG');
      assert.strictEqual(base, 'debug-workflow');
      assert.strictEqual(params.investigator_level, 'level2');
    });

    it('SIMPLE TASK should use worker-validator', function () {
      const { base } = getConfig('SIMPLE', 'TASK');
      assert.strictEqual(base, 'worker-validator');
    });

    it('STANDARD should use full-workflow with 2 validators', function () {
      const { base, params } = getConfig('STANDARD', 'TASK');
      assert.strictEqual(base, 'full-workflow');
      assert.strictEqual(params.validator_count, 2);
      assert.strictEqual(params.planner_level, 'level2');
    });

    it('CRITICAL should use level3 planner and 4 validators', function () {
      const { base, params } = getConfig('CRITICAL', 'TASK');
      assert.strictEqual(base, 'full-workflow');
      assert.strictEqual(params.planner_level, 'level3');
      assert.strictEqual(params.validator_count, 4);
      assert.strictEqual(params.max_iterations, DEFAULT_MAX_ITERATIONS);
    });

    it('TRIVIAL DEBUG should still use single-worker (not debug-workflow)', function () {
      const { base } = getConfig('TRIVIAL', 'DEBUG');
      assert.strictEqual(base, 'single-worker');
    });
  });

  describe('Feedback loops preserved', function () {
    it('worker-validator should have rejection trigger', function () {
      const { base, params } = getConfig('SIMPLE', 'TASK');
      const resolved = resolver.resolve(base, params);

      const worker = resolved.agents.find((a) => a.id === 'worker');
      const rejectionTrigger = worker.triggers.find(
        (t) => t.topic === 'VALIDATION_RESULT' && t.logic
      );

      assert.ok(rejectionTrigger, 'Worker should have rejection trigger');
      assert.ok(
        rejectionTrigger.logic.script.includes('approved === false'),
        'Trigger should check for rejection'
      );
    });

    it('debug-workflow should have rejection trigger on fixer', function () {
      const { base, params } = getConfig('SIMPLE', 'DEBUG');
      const resolved = resolver.resolve(base, params);

      const fixer = resolved.agents.find((a) => a.id === 'fixer');
      const rejectionTrigger = fixer.triggers.find(
        (t) => t.topic === 'VALIDATION_RESULT' && t.logic
      );

      assert.ok(rejectionTrigger, 'Fixer should have rejection trigger');
    });

    it('full-workflow should have consensus-based rejection trigger', function () {
      const { base, params } = getConfig('CRITICAL', 'TASK');
      const resolved = resolver.resolve(base, params);

      const worker = resolved.agents.find((a) => a.id === 'worker');
      const rejectionTrigger = worker.triggers.find(
        (t) => t.topic === 'VALIDATION_RESULT' && t.logic
      );

      assert.ok(rejectionTrigger, 'Worker should have rejection trigger');
      assert.ok(
        rejectionTrigger.logic.script.includes('getAgentsByRole'),
        'Should check all validators'
      );
    });
  });
});
