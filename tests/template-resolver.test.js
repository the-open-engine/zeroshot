/**
 * Tests for TemplateResolver and 3D classification routing
 */

const assert = require('assert');
const path = require('path');
const TemplateResolver = require('../src/template-resolver');

// Copy of getConfig logic from logic-engine.js for testing
function getConfig(domain, complexity, taskType) {
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

  const getModel = (role) => {
    if (complexity === 'CRITICAL' && role === 'planner') return 'opus';
    if (complexity === 'TRIVIAL') return 'haiku';
    return 'sonnet';
  };

  const getValidatorCount = () => {
    if (complexity === 'TRIVIAL') return 0;
    if (complexity === 'SIMPLE') return 1;
    if (complexity === 'STANDARD') return 2;
    if (complexity === 'CRITICAL') return 4;
    return 1;
  };

  const getMaxIterations = () => {
    if (complexity === 'TRIVIAL') return 1;
    if (complexity === 'SIMPLE') return 3;
    if (complexity === 'STANDARD') return 5;
    if (complexity === 'CRITICAL') return 7;
    return 3;
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
    domain,
    task_type: taskType,
    complexity,
    max_tokens: getMaxTokens(),
    max_iterations: getMaxIterations(),
  };

  if (base === 'single-worker') {
    params.worker_model = getModel('worker');
  } else if (base === 'worker-validator') {
    params.worker_model = getModel('worker');
    params.validator_model = getModel('validator');
  } else if (base === 'debug-workflow') {
    params.investigator_model = getModel('planner');
    params.fixer_model = getModel('worker');
    params.tester_model = getModel('validator');
  } else if (base === 'full-workflow') {
    params.planner_model = getModel('planner');
    params.worker_model = getModel('worker');
    params.validator_model = getModel('validator');
    params.validator_count = getValidatorCount();
  }

  return { base, params };
}

const DOMAINS = ['CODE', 'INFRA', 'CICD', 'OPS', 'TESTING', 'CONTEXT'];
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
      assert.ok(info.params.worker_model);
    });

    it('should return null for non-existent template', function () {
      const info = resolver.getTemplateInfo('does-not-exist');
      assert.strictEqual(info, null);
    });
  });

  describe('resolve', function () {
    it('should resolve single-worker template', function () {
      const resolved = resolver.resolve('single-worker', {
        domain: 'CODE',
        task_type: 'TASK',
        complexity: 'TRIVIAL',
        max_tokens: 50000,
        max_iterations: 1,
        worker_model: 'haiku',
      });

      assert.ok(resolved.agents);
      assert.strictEqual(resolved.agents.length, 2); // worker + completion-detector

      const worker = resolved.agents.find((a) => a.id === 'worker');
      assert.strictEqual(worker.model, 'haiku');
    });

    it('should resolve full-workflow with conditional validators', function () {
      const resolved = resolver.resolve('full-workflow', {
        domain: 'CODE',
        task_type: 'TASK',
        complexity: 'CRITICAL',
        max_tokens: 150000,
        max_iterations: 7,
        planner_model: 'opus',
        worker_model: 'sonnet',
        validator_model: 'sonnet',
        validator_count: 4,
      });

      assert.ok(resolved.agents);

      const planner = resolved.agents.find((a) => a.id === 'planner');
      assert.strictEqual(planner.model, 'opus');

      // Should have 4 validators for CRITICAL
      const validators = resolved.agents.filter((a) => a.role === 'validator');
      assert.strictEqual(validators.length, 4);
    });

    it('should fail on missing required params', function () {
      assert.throws(() => resolver.resolve('single-worker', {}), /Missing required params/);
    });

    it('should fail on non-existent template', function () {
      assert.throws(() => resolver.resolve('does-not-exist', {}), /Base template not found/);
    });
  });
});

describe('3D Classification Routing', function () {
  let resolver;

  before(function () {
    const templatesDir = path.join(__dirname, '..', 'cluster-templates');
    resolver = new TemplateResolver(templatesDir);
  });

  describe('All 72 combinations', function () {
    for (const domain of DOMAINS) {
      for (const complexity of COMPLEXITIES) {
        for (const taskType of TASK_TYPES) {
          const key = `${domain}:${complexity}:${taskType}`;

          it(`should resolve ${key}`, function () {
            const { base, params } = getConfig(domain, complexity, taskType);
            const resolved = resolver.resolve(base, params);

            assert.ok(resolved.agents, `${key}: No agents`);
            assert.ok(resolved.agents.length > 0, `${key}: Empty agents array`);

            // Verify orchestrator exists
            const hasOrchestrator = resolved.agents.some(
              (a) => a.role === 'orchestrator' || a.id === 'completion-detector'
            );
            assert.ok(hasOrchestrator, `${key}: No orchestrator`);
          });
        }
      }
    }
  });

  describe('Classification correctness', function () {
    it('TRIVIAL should use single-worker with haiku', function () {
      const { base, params } = getConfig('CODE', 'TRIVIAL', 'TASK');
      assert.strictEqual(base, 'single-worker');
      assert.strictEqual(params.worker_model, 'haiku');
    });

    it('SIMPLE DEBUG should use debug-workflow', function () {
      const { base, params } = getConfig('CODE', 'SIMPLE', 'DEBUG');
      assert.strictEqual(base, 'debug-workflow');
      assert.strictEqual(params.investigator_model, 'sonnet');
    });

    it('SIMPLE TASK should use worker-validator', function () {
      const { base } = getConfig('CODE', 'SIMPLE', 'TASK');
      assert.strictEqual(base, 'worker-validator');
    });

    it('STANDARD should use full-workflow with 2 validators', function () {
      const { base, params } = getConfig('CODE', 'STANDARD', 'TASK');
      assert.strictEqual(base, 'full-workflow');
      assert.strictEqual(params.validator_count, 2);
      assert.strictEqual(params.planner_model, 'sonnet');
    });

    it('CRITICAL should use opus planner and 4 validators', function () {
      const { base, params } = getConfig('CODE', 'CRITICAL', 'TASK');
      assert.strictEqual(base, 'full-workflow');
      assert.strictEqual(params.planner_model, 'opus');
      assert.strictEqual(params.validator_count, 4);
      assert.strictEqual(params.max_iterations, 7);
    });

    it('TRIVIAL DEBUG should still use single-worker (not debug-workflow)', function () {
      const { base } = getConfig('CODE', 'TRIVIAL', 'DEBUG');
      assert.strictEqual(base, 'single-worker');
    });

    it('Domain should be injected into params', function () {
      const { params: codeParams } = getConfig('CODE', 'SIMPLE', 'TASK');
      const { params: infraParams } = getConfig('INFRA', 'SIMPLE', 'TASK');

      assert.strictEqual(codeParams.domain, 'CODE');
      assert.strictEqual(infraParams.domain, 'INFRA');
    });
  });

  describe('Feedback loops preserved', function () {
    it('worker-validator should have rejection trigger', function () {
      const { base, params } = getConfig('CODE', 'SIMPLE', 'TASK');
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
      const { base, params } = getConfig('CODE', 'SIMPLE', 'DEBUG');
      const resolved = resolver.resolve(base, params);

      const fixer = resolved.agents.find((a) => a.id === 'fixer');
      const rejectionTrigger = fixer.triggers.find(
        (t) => t.topic === 'VALIDATION_RESULT' && t.logic
      );

      assert.ok(rejectionTrigger, 'Fixer should have rejection trigger');
    });

    it('full-workflow should have consensus-based rejection trigger', function () {
      const { base, params } = getConfig('CODE', 'CRITICAL', 'TASK');
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
