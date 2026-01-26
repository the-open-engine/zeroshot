/**
 * Tests for two-stage validation pipeline (quick → heavy)
 */

const assert = require('assert');
const path = require('path');
const TemplateResolver = require('../src/template-resolver');

describe('Two-Stage Validation Pipeline', function () {
  let resolver;

  before(function () {
    const templatesDir = path.join(__dirname, '..', 'cluster-templates');
    resolver = new TemplateResolver(templatesDir);
  });

  describe('quick-validation template', function () {
    it('should contain validator-requirements and validator-code', function () {
      const resolved = resolver.resolve('quick-validation', {});
      assert.ok(resolved.agents, 'Template should have agents');

      const requirements = resolved.agents.find((a) => a.id === 'validator-requirements');
      assert.ok(requirements, 'validator-requirements should exist');

      const code = resolved.agents.find((a) => a.id === 'validator-code');
      assert.ok(code, 'validator-code should exist');
    });

    it('should trigger on IMPLEMENTATION_READY', function () {
      const resolved = resolver.resolve('quick-validation', {});

      const requirements = resolved.agents.find((a) => a.id === 'validator-requirements');
      const trigger = requirements.triggers.find((t) => t.topic === 'IMPLEMENTATION_READY');
      assert.ok(trigger, 'validator-requirements should trigger on IMPLEMENTATION_READY');

      const code = resolved.agents.find((a) => a.id === 'validator-code');
      const codeTrigger = code.triggers.find((t) => t.topic === 'IMPLEMENTATION_READY');
      assert.ok(codeTrigger, 'validator-code should trigger on IMPLEMENTATION_READY');
    });

    it('should have consensus-coordinator publishing QUICK_VALIDATION_PASSED on success', function () {
      const resolved = resolver.resolve('quick-validation', {});

      const coordinator = resolved.agents.find((a) => a.id === 'consensus-coordinator');
      assert.ok(coordinator, 'consensus-coordinator should exist');

      const hook = coordinator.hooks.onComplete;
      assert.ok(hook, 'consensus-coordinator should have onComplete hook');
      assert.strictEqual(hook.action, 'publish_message');
    });

    it('should publish VALIDATION_RESULT if any validator rejects', function () {
      const resolved = resolver.resolve('quick-validation', {});

      const coordinator = resolved.agents.find((a) => a.id === 'consensus-coordinator');
      const logicScript = coordinator.triggers.find((t) => t.logic)?.logic?.script;

      assert.ok(logicScript, 'consensus-coordinator should have logic script');
      assert.ok(
        logicScript.includes('VALIDATION_RESULT'),
        'Logic should publish VALIDATION_RESULT on rejection'
      );
    });
  });

  describe('heavy-validation template', function () {
    it('should contain validator-security and validator-tester', function () {
      const resolved = resolver.resolve('heavy-validation', {});
      assert.ok(resolved.agents, 'Template should have agents');

      const security = resolved.agents.find((a) => a.id === 'validator-security');
      assert.ok(security, 'validator-security should exist');

      const tester = resolved.agents.find((a) => a.id === 'validator-tester');
      assert.ok(tester, 'validator-tester should exist');
    });

    it('should trigger on QUICK_VALIDATION_PASSED', function () {
      const resolved = resolver.resolve('heavy-validation', {});

      const security = resolved.agents.find((a) => a.id === 'validator-security');
      const trigger = security.triggers.find((t) => t.topic === 'QUICK_VALIDATION_PASSED');
      assert.ok(trigger, 'validator-security should trigger on QUICK_VALIDATION_PASSED');

      const tester = resolved.agents.find((a) => a.id === 'validator-tester');
      const testerTrigger = tester.triggers.find((t) => t.topic === 'QUICK_VALIDATION_PASSED');
      assert.ok(testerTrigger, 'validator-tester should trigger on QUICK_VALIDATION_PASSED');
    });

    it('should have consensus-coordinator publishing VALIDATION_RESULT', function () {
      const resolved = resolver.resolve('heavy-validation', {});

      const coordinator = resolved.agents.find((a) => a.id === 'consensus-coordinator');
      assert.ok(coordinator, 'consensus-coordinator should exist');

      const hook = coordinator.hooks.onComplete;
      assert.ok(hook, 'consensus-coordinator should have onComplete hook');
      assert.strictEqual(hook.action, 'publish_message');

      const logicScript = coordinator.triggers.find((t) => t.logic)?.logic?.script;
      assert.ok(
        logicScript.includes('VALIDATION_RESULT'),
        'Logic should publish VALIDATION_RESULT'
      );
    });

    it('should have contextStrategy for QUICK_VALIDATION_PASSED', function () {
      const resolved = resolver.resolve('heavy-validation', {});

      const security = resolved.agents.find((a) => a.id === 'validator-security');
      const contextSource = security.contextStrategy?.sources?.find(
        (s) => s.topic === 'QUICK_VALIDATION_PASSED'
      );

      assert.ok(contextSource, 'validator-security should have QUICK_VALIDATION_PASSED context');
      assert.strictEqual(contextSource.priority, 'required');
    });
  });

  describe('full-workflow integration', function () {
    it('should load meta-coordinator for CRITICAL tasks', function () {
      const resolved = resolver.resolve('full-workflow', {
        task_type: 'TASK',
        complexity: 'CRITICAL',
        max_tokens: 150000,
        max_iterations: 25,
        planner_level: 'level3',
        worker_level: 'level2',
        validator_level: 'level2',
        validator_count: 0,
      });

      const metaCoordinator = resolved.agents.find((a) => a.id === 'meta-coordinator');
      assert.ok(metaCoordinator, 'meta-coordinator should be present for CRITICAL tasks');

      // Inline validators should be filtered out
      const validators = resolved.agents.filter((a) => a.role === 'validator');
      assert.strictEqual(validators.length, 0, 'No inline validators for CRITICAL tasks');
    });

    it('should NOT load meta-coordinator for STANDARD tasks', function () {
      const resolved = resolver.resolve('full-workflow', {
        task_type: 'TASK',
        complexity: 'STANDARD',
        max_tokens: 100000,
        max_iterations: 25,
        planner_level: 'level2',
        worker_level: 'level2',
        validator_level: 'level2',
        validator_count: 2,
      });

      const metaCoordinator = resolved.agents.find((a) => a.id === 'meta-coordinator');
      assert.ok(!metaCoordinator, 'meta-coordinator should NOT be present for STANDARD tasks');

      // Inline validators should be present
      const validators = resolved.agents.filter((a) => a.role === 'validator');
      assert.strictEqual(validators.length, 2, 'STANDARD tasks should have 2 inline validators');
    });
  });

  describe('Sequential execution order', function () {
    it('Stage 2 cannot trigger without QUICK_VALIDATION_PASSED', function () {
      const heavyResolved = resolver.resolve('heavy-validation', {});

      // Heavy validators ONLY trigger on QUICK_VALIDATION_PASSED
      const heavySecurity = heavyResolved.agents.find((a) => a.id === 'validator-security');
      const triggers = heavySecurity.triggers.filter((t) => t.topic !== 'QUICK_VALIDATION_PASSED');

      assert.strictEqual(
        triggers.length,
        0,
        'Heavy validators should ONLY trigger on QUICK_VALIDATION_PASSED'
      );
    });

    it('Consensus-coordinator publishes VALIDATION_RESULT, not QUICK_VALIDATION_PASSED on rejection', function () {
      const resolved = resolver.resolve('quick-validation', {});

      const coordinator = resolved.agents.find((a) => a.id === 'consensus-coordinator');
      const hookLogic = coordinator.hooks.onComplete?.logic?.script;

      assert.ok(hookLogic, 'consensus-coordinator should have hook logic script');
      assert.ok(
        hookLogic.includes('!result.allApproved') ||
          hookLogic.includes('result.approved === false'),
        'Logic should check for rejections'
      );
      assert.ok(
        hookLogic.includes('VALIDATION_RESULT'),
        'Should publish VALIDATION_RESULT on rejection (skips QUICK_VALIDATION_PASSED)'
      );
    });
  });
});
