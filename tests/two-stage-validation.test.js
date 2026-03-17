/**
 * Tests for two-stage validation pipeline (quick → heavy)
 */

const assert = require('assert');
const path = require('path');
const TemplateResolver = require('../src/template-resolver');
const { validateConfig } = require('../src/config-validator');
const LogicEngine = require('../src/logic-engine');
const MessageBus = require('../src/message-bus');
const Ledger = require('../src/ledger');

/**
 * Helper: create mock cluster with validators having different hook topics
 */
function createMixedValidatorCluster() {
  return {
    id: 'heavy-regression',
    agents: [
      {
        id: 'validator-requirements',
        role: 'validator',
        hooks: {
          onComplete: { action: 'publish_message', config: { topic: 'QUICK_VALIDATION_RESULT' } },
        },
      },
      {
        id: 'validator-code',
        role: 'validator',
        hooks: {
          onComplete: { action: 'publish_message', config: { topic: 'QUICK_VALIDATION_RESULT' } },
        },
      },
      {
        id: 'validator-security',
        role: 'validator',
        hooks: {
          onComplete: { action: 'publish_message', config: { topic: 'HEAVY_VALIDATION_RESULT' } },
        },
      },
      {
        id: 'validator-tester',
        role: 'validator',
        hooks: {
          onComplete: { action: 'publish_message', config: { topic: 'HEAVY_VALIDATION_RESULT' } },
        },
      },
      {
        id: 'validator-context',
        role: 'validator',
        hooks: {
          onComplete: { action: 'publish_message', config: { topic: 'VALIDATION_RESULT' } },
        },
      },
      { id: 'consensus-coordinator', role: 'coordinator' },
    ],
  };
}

/**
 * Helper: publish sequence of validation messages for regression test
 */
function publishRegressionSequence(messageBus, cluster) {
  let ts = Date.now();
  const nextTs = () => ++ts;

  messageBus.publish({
    cluster_id: cluster.id,
    topic: 'QUICK_VALIDATION_PASSED',
    sender: 'consensus-coordinator',
    timestamp: nextTs(),
  });

  messageBus.publish({
    cluster_id: cluster.id,
    topic: 'HEAVY_VALIDATION_RESULT',
    sender: 'validator-security',
    timestamp: nextTs(),
    content: { data: { approved: true } },
  });

  return { messageBus, cluster, nextTs };
}

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
    it('should contain validator-security and validator-tester by default', function () {
      const resolved = resolver.resolve('heavy-validation', {});
      assert.ok(resolved.agents, 'Template should have agents');

      const security = resolved.agents.find((a) => a.id === 'validator-security');
      assert.ok(security, 'validator-security should exist');

      const tester = resolved.agents.find((a) => a.id === 'validator-tester');
      assert.ok(tester, 'validator-tester should exist');

      const runtime = resolved.agents.find((a) => a.id === 'validator-runtime');
      assert.ok(
        !runtime,
        'validator-runtime should be omitted when runtime validation is disabled'
      );
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

    it('should add validator-runtime when runtime validation is enabled', function () {
      const resolved = resolver.resolve('heavy-validation', {
        include_runtime_validator: true,
        heavy_validator_count: 3,
      });

      const runtime = resolved.agents.find((a) => a.id === 'validator-runtime');
      assert.ok(runtime, 'validator-runtime should exist when enabled');
      assert.strictEqual(runtime.requiresValidationRuntime, true);

      const trigger = runtime.triggers.find((t) => t.topic === 'QUICK_VALIDATION_PASSED');
      assert.ok(trigger, 'validator-runtime should trigger on QUICK_VALIDATION_PASSED');

      const hookTopic = runtime.hooks?.onComplete?.config?.topic;
      assert.strictEqual(hookTopic, 'HEAVY_VALIDATION_RESULT');
    });

    it('should update heavy consensus to wait for validator-runtime when enabled', function () {
      const resolved = resolver.resolve('heavy-validation', {
        include_runtime_validator: true,
        heavy_validator_count: 3,
      });

      const coordinator = resolved.agents.find((a) => a.id === 'consensus-coordinator');
      const source = coordinator.contextStrategy.sources.find(
        (entry) => entry.topic === 'HEAVY_VALIDATION_RESULT'
      );
      assert.strictEqual(source.amount, 3, 'heavy consensus should expect 3 validator results');

      const triggerScript = coordinator.triggers.find((t) => t.topic === 'HEAVY_VALIDATION_RESULT')
        ?.logic?.script;
      assert.ok(
        triggerScript.includes('HEAVY_VALIDATION_RESULT'),
        'heavy consensus should derive the active validator set from heavy validator outputs'
      );
      assert.ok(
        triggerScript.includes('candidate?.config?.hooks?.onComplete?.config?.topic'),
        'heavy consensus should inspect agent hook topics so stage-1 validators do not block stage 2'
      );
    });

    it('should ignore stage-1 validators when waiting for heavy validation results', function () {
      const resolved = resolver.resolve('heavy-validation', {});
      const coordinator = resolved.agents.find((a) => a.id === 'consensus-coordinator');
      const triggerScript = coordinator?.triggers?.find(
        (t) => t.topic === 'HEAVY_VALIDATION_RESULT'
      )?.logic?.script;
      assert.ok(triggerScript, 'heavy consensus trigger script should exist');

      const cluster = createMixedValidatorCluster();
      const ledger = new Ledger(':memory:');
      const messageBus = new MessageBus(ledger);
      const logicEngine = new LogicEngine(messageBus, cluster);

      try {
        const { nextTs } = publishRegressionSequence(messageBus, cluster);

        let shouldTrigger = logicEngine.evaluate(
          triggerScript,
          { id: 'consensus-coordinator', cluster_id: cluster.id },
          { topic: 'HEAVY_VALIDATION_RESULT' }
        );
        assert.strictEqual(shouldTrigger, false, 'must wait for both validators');

        messageBus.publish({
          cluster_id: cluster.id,
          topic: 'HEAVY_VALIDATION_RESULT',
          sender: 'validator-tester',
          timestamp: nextTs(),
          content: { data: { approved: true } },
        });

        shouldTrigger = logicEngine.evaluate(
          triggerScript,
          { id: 'consensus-coordinator', cluster_id: cluster.id },
          { topic: 'HEAVY_VALIDATION_RESULT' }
        );
        assert.strictEqual(shouldTrigger, true, 'should trigger once both validators respond');

        messageBus.publish({
          cluster_id: cluster.id,
          topic: 'VALIDATION_RESULT',
          sender: 'consensus-coordinator',
          timestamp: nextTs(),
          content: { data: { approved: true, stage: 'heavy' } },
        });

        messageBus.publish({
          cluster_id: cluster.id,
          topic: 'HEAVY_VALIDATION_RESULT',
          sender: 'validator-security',
          timestamp: nextTs(),
          content: { data: { approved: false } },
        });

        shouldTrigger = logicEngine.evaluate(
          triggerScript,
          { id: 'consensus-coordinator', cluster_id: cluster.id },
          { topic: 'HEAVY_VALIDATION_RESULT' }
        );
        assert.strictEqual(
          shouldTrigger,
          false,
          'late update from one validator must not retrigger heavy consensus'
        );

        messageBus.publish({
          cluster_id: cluster.id,
          topic: 'HEAVY_VALIDATION_RESULT',
          sender: 'validator-tester',
          timestamp: nextTs(),
          content: { data: { approved: false } },
        });

        shouldTrigger = logicEngine.evaluate(
          triggerScript,
          { id: 'consensus-coordinator', cluster_id: cluster.id },
          { topic: 'HEAVY_VALIDATION_RESULT' }
        );
        assert.strictEqual(
          shouldTrigger,
          true,
          'should trigger again only after both validators publish a fresh cycle'
        );
      } finally {
        ledger.close();
      }
    });

    it('should ignore stage-1 quick validators when evaluating heavy validation completion', function () {
      const resolved = resolver.resolve('heavy-validation', {});
      const coordinator = resolved.agents.find((a) => a.id === 'consensus-coordinator');
      const triggerScript = coordinator?.triggers?.find(
        (t) => t.topic === 'HEAVY_VALIDATION_RESULT'
      )?.logic?.script;
      assert.ok(triggerScript, 'heavy consensus trigger script should exist');

      const cluster = createMixedValidatorCluster();
      const ledger = new Ledger(':memory:');
      const messageBus = new MessageBus(ledger);
      const logicEngine = new LogicEngine(messageBus, cluster);

      try {
        const { nextTs } = publishRegressionSequence(messageBus, cluster);

        messageBus.publish({
          cluster_id: cluster.id,
          topic: 'QUICK_VALIDATION_RESULT',
          sender: 'validator-requirements',
          timestamp: nextTs(),
          content: { data: { approved: true } },
        });

        messageBus.publish({
          cluster_id: cluster.id,
          topic: 'QUICK_VALIDATION_RESULT',
          sender: 'validator-code',
          timestamp: nextTs(),
          content: { data: { approved: true } },
        });

        let shouldTrigger = logicEngine.evaluate(
          triggerScript,
          { id: 'consensus-coordinator', cluster_id: cluster.id },
          { topic: 'HEAVY_VALIDATION_RESULT' }
        );
        assert.strictEqual(
          shouldTrigger,
          false,
          'quick validation results should NOT count toward heavy validation completion'
        );

        messageBus.publish({
          cluster_id: cluster.id,
          topic: 'HEAVY_VALIDATION_RESULT',
          sender: 'validator-tester',
          timestamp: nextTs(),
          content: { data: { approved: true } },
        });

        shouldTrigger = logicEngine.evaluate(
          triggerScript,
          { id: 'consensus-coordinator', cluster_id: cluster.id },
          { topic: 'HEAVY_VALIDATION_RESULT' }
        );
        assert.strictEqual(
          shouldTrigger,
          true,
          'should trigger once both HEAVY validators respond (ignoring quick validators)'
        );
      } finally {
        ledger.close();
      }
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

      // Regression: config-validator should NOT raise Gap 15 role-reference errors when validators are absent.
      const validation = validateConfig(resolved);
      const roleErrors = validation.errors.filter(
        (e) =>
          e.includes('[Gap 15]') ||
          e.includes("Logic references role 'validator'") ||
          e.includes('Logic references role "validator"')
      );
      assert.strictEqual(roleErrors.length, 0, `Unexpected role reference errors: ${roleErrors}`);
    });

    it('meta-coordinator should republish trigger topic after load_config (prevents validator deadlock)', function () {
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

      const hookTransformScript = metaCoordinator.hooks?.onComplete?.transform?.script || '';
      assert.ok(
        hookTransformScript.includes("action: 'publish'") ||
          hookTransformScript.includes('action: "publish"'),
        'meta-coordinator should publish a republished trigger topic after load_config'
      );
      assert.ok(
        hookTransformScript.includes('_republished'),
        'meta-coordinator republish should include _republished metadata'
      );

      const implTrigger = metaCoordinator.triggers?.find((t) => t.topic === 'IMPLEMENTATION_READY');
      assert.ok(implTrigger?.logic?.script?.includes('_republished'));

      const stage2Trigger = metaCoordinator.triggers?.find(
        (t) => t.topic === 'QUICK_VALIDATION_PASSED'
      );
      assert.ok(stage2Trigger?.logic?.script?.includes('_republished'));
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
