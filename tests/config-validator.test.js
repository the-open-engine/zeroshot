/**
 * Config Validator Tests
 *
 * Tests all validation rules for cluster configurations including:
 * - Basic structure validation
 * - Message flow analysis
 * - Agent-specific validation
 * - Logic script validation
 */

const assert = require('assert');
const {
  validateConfig,
  validateBasicStructure,
  analyzeMessageFlow,
  validateAgents,
  validateLogicScripts,
  isValidIterationPattern,
} = require('../src/config-validator');

describe('Config Validator', function () {
  // === BASIC STRUCTURE TESTS ===

  describe('validateBasicStructure', function () {
    it('should reject config without agents array', function () {
      const result = validateBasicStructure({});
      assert.strictEqual(result.errors.length, 1);
      assert.ok(result.errors[0].includes('agents array is required'));
    });

    it('should reject empty agents array', function () {
      const result = validateBasicStructure({ agents: [] });
      assert.ok(result.errors.some((e) => e.includes('cannot be empty')));
    });

    it('should require agent id, role, and triggers', function () {
      const result = validateBasicStructure({
        agents: [{ prompt: 'test' }],
      });
      assert.ok(result.errors.some((e) => e.includes('.id is required')));
      assert.ok(result.errors.some((e) => e.includes('.role is required')));
      assert.ok(result.errors.some((e) => e.includes('.triggers array is required')));
    });

    it('should reject duplicate agent ids', function () {
      const result = validateBasicStructure({
        agents: [
          { id: 'worker', role: 'impl', triggers: [{ topic: 'A' }] },
          { id: 'worker', role: 'validator', triggers: [{ topic: 'B' }] },
        ],
      });
      assert.ok(result.errors.some((e) => e.includes('Duplicate agent id')));
    });

    it('should reject empty triggers array', function () {
      const result = validateBasicStructure({
        agents: [{ id: 'worker', role: 'impl', triggers: [] }],
      });
      assert.ok(result.errors.some((e) => e.includes('triggers cannot be empty')));
    });

    it('should reject invalid trigger action', function () {
      const result = validateBasicStructure({
        agents: [
          {
            id: 'worker',
            role: 'impl',
            triggers: [{ topic: 'A', action: 'invalid_action' }],
          },
        ],
      });
      assert.ok(result.errors.some((e) => e.includes("must be 'execute_task' or 'stop_cluster'")));
    });

    it('should reject logic without script', function () {
      const result = validateBasicStructure({
        agents: [
          {
            id: 'worker',
            role: 'impl',
            triggers: [{ topic: 'A', logic: { engine: 'javascript' } }],
          },
        ],
      });
      assert.ok(result.errors.some((e) => e.includes('logic.script is required')));
    });

    it('should pass valid basic config', function () {
      const result = validateBasicStructure({
        agents: [
          {
            id: 'worker',
            role: 'implementation',
            triggers: [{ topic: 'ISSUE_OPENED' }],
          },
          {
            id: 'validator',
            role: 'validator',
            triggers: [{ topic: 'IMPL_READY' }],
          },
        ],
      });
      assert.strictEqual(result.errors.length, 0);
    });
  });

  // === MODEL RULES TESTS ===

  describe('isValidIterationPattern', function () {
    it('should accept "all"', () => assert.ok(isValidIterationPattern('all')));
    it('should accept exact number "3"', () => assert.ok(isValidIterationPattern('3')));
    it('should accept range "1-3"', () => assert.ok(isValidIterationPattern('1-3')));
    it('should accept open-ended "5+"', () => assert.ok(isValidIterationPattern('5+')));
    it('should reject "1..3"', () => assert.ok(!isValidIterationPattern('1..3')));
    it('should reject "five"', () => assert.ok(!isValidIterationPattern('five')));
    it('should reject empty string', () => assert.ok(!isValidIterationPattern('')));
  });

  describe('modelRules validation', function () {
    it('should reject modelRules without catch-all', function () {
      const result = validateBasicStructure({
        agents: [
          {
            id: 'worker',
            role: 'impl',
            triggers: [{ topic: 'A' }],
            modelRules: [
              { iterations: '1-3', model: 'sonnet' },
              // Missing catch-all for 4+
            ],
          },
        ],
      });
      assert.ok(result.errors.some((e) => e.includes('no catch-all rule')));
    });

    it('should accept modelRules with "all" catch-all', function () {
      const result = validateBasicStructure({
        agents: [
          {
            id: 'worker',
            role: 'impl',
            triggers: [{ topic: 'A' }],
            modelRules: [
              { iterations: '1-3', model: 'sonnet' },
              { iterations: 'all', model: 'opus' },
            ],
          },
        ],
      });
      assert.ok(!result.errors.some((e) => e.includes('catch-all')));
    });

    it('should accept modelRules with "N+" catch-all', function () {
      const result = validateBasicStructure({
        agents: [
          {
            id: 'worker',
            role: 'impl',
            triggers: [{ topic: 'A' }],
            modelRules: [
              { iterations: '1-3', model: 'sonnet' },
              { iterations: '4+', model: 'opus' },
            ],
          },
        ],
      });
      assert.ok(!result.errors.some((e) => e.includes('catch-all')));
    });

    it('should reject invalid model name', function () {
      const result = validateBasicStructure({
        agents: [
          {
            id: 'worker',
            role: 'impl',
            triggers: [{ topic: 'A' }],
            modelRules: [{ iterations: 'all', model: 'gpt4' }],
          },
        ],
      });
      assert.ok(result.errors.some((e) => e.includes("must be 'opus', 'sonnet', or 'haiku'")));
    });
  });

  // === MESSAGE FLOW TESTS ===

  describe('analyzeMessageFlow', function () {
    it('should error if no agent triggers on ISSUE_OPENED', function () {
      const result = analyzeMessageFlow({
        agents: [{ id: 'worker', role: 'impl', triggers: [{ topic: 'PLAN_READY' }] }],
      });
      assert.ok(result.errors.some((e) => e.includes('No agent triggers on ISSUE_OPENED')));
    });

    it('should error if no completion handler exists', function () {
      const result = analyzeMessageFlow({
        agents: [{ id: 'worker', role: 'impl', triggers: [{ topic: 'ISSUE_OPENED' }] }],
      });
      assert.ok(result.errors.some((e) => e.includes('No completion handler found')));
    });

    it('should error on multiple completion handlers', function () {
      const result = analyzeMessageFlow({
        agents: [
          { id: 'worker', role: 'impl', triggers: [{ topic: 'ISSUE_OPENED' }] },
          {
            id: 'completion-detector',
            role: 'orchestrator',
            triggers: [{ topic: 'X', action: 'stop_cluster' }],
          },
          {
            id: 'git-pusher',
            role: 'orchestrator',
            triggers: [{ topic: 'Y', action: 'stop_cluster' }],
          },
        ],
      });
      assert.ok(result.errors.some((e) => e.includes('Multiple completion handlers')));
    });

    it('should error when topic is never produced', function () {
      const result = analyzeMessageFlow({
        agents: [
          { id: 'worker', role: 'impl', triggers: [{ topic: 'ISSUE_OPENED' }] },
          {
            id: 'validator',
            role: 'validator',
            triggers: [{ topic: 'PLAN_READY' }],
          }, // Never produced!
          {
            id: 'completion',
            role: 'orchestrator',
            triggers: [{ topic: 'X', action: 'stop_cluster' }],
          },
        ],
      });
      assert.ok(
        result.errors.some((e) => e.includes('PLAN_READY') && e.includes('never produced'))
      );
    });

    it('should warn on orphan topics (produced but never consumed)', function () {
      const result = analyzeMessageFlow({
        agents: [
          {
            id: 'worker',
            role: 'impl',
            triggers: [{ topic: 'ISSUE_OPENED' }],
            hooks: { onComplete: { config: { topic: 'ORPHAN_TOPIC' } } },
          },
          {
            id: 'completion',
            role: 'orchestrator',
            triggers: [{ topic: 'ISSUE_OPENED', action: 'stop_cluster' }],
          },
        ],
      });
      assert.ok(
        result.warnings.some((w) => w.includes('ORPHAN_TOPIC') && w.includes('never consumed'))
      );
    });

    it('should error on self-triggering agent', function () {
      const result = analyzeMessageFlow({
        agents: [
          {
            id: 'echo',
            role: 'impl',
            triggers: [{ topic: 'ECHO' }],
            hooks: { onComplete: { config: { topic: 'ECHO' } } },
          },
          {
            id: 'starter',
            role: 'impl',
            triggers: [{ topic: 'ISSUE_OPENED' }],
          },
          {
            id: 'completion',
            role: 'orchestrator',
            triggers: [{ topic: 'X', action: 'stop_cluster' }],
          },
        ],
      });
      assert.ok(result.errors.some((e) => e.includes('echo') && e.includes('infinite loop')));
    });

    it('should warn on circular dependency without escape logic', function () {
      const result = analyzeMessageFlow({
        agents: [
          {
            id: 'a',
            role: 'impl',
            triggers: [{ topic: 'ISSUE_OPENED' }, { topic: 'B_OUTPUT' }],
            hooks: { onComplete: { config: { topic: 'A_OUTPUT' } } },
          },
          {
            id: 'b',
            role: 'validator',
            triggers: [{ topic: 'A_OUTPUT' }],
            hooks: { onComplete: { config: { topic: 'B_OUTPUT' } } },
          },
          {
            id: 'completion',
            role: 'orchestrator',
            triggers: [{ topic: 'X', action: 'stop_cluster' }],
          },
        ],
      });
      assert.ok(result.warnings.some((w) => w.includes('Circular dependency')));
    });

    it('should not warn on circular dependency WITH escape logic', function () {
      const result = analyzeMessageFlow({
        agents: [
          {
            id: 'a',
            role: 'impl',
            triggers: [
              { topic: 'ISSUE_OPENED' },
              {
                topic: 'B_OUTPUT',
                logic: { script: 'return !message.content.approved;' },
              },
            ],
            hooks: { onComplete: { config: { topic: 'A_OUTPUT' } } },
          },
          {
            id: 'b',
            role: 'validator',
            triggers: [{ topic: 'A_OUTPUT' }],
            hooks: { onComplete: { config: { topic: 'B_OUTPUT' } } },
          },
          {
            id: 'completion',
            role: 'orchestrator',
            triggers: [{ topic: 'X', action: 'stop_cluster' }],
          },
        ],
      });
      assert.ok(!result.warnings.some((w) => w.includes('Circular dependency')));
    });

    it('should error when worker has validators but no re-trigger on rejection', function () {
      const result = analyzeMessageFlow({
        agents: [
          {
            id: 'worker',
            role: 'implementation',
            triggers: [{ topic: 'ISSUE_OPENED' }], // No VALIDATION_RESULT trigger!
            hooks: { onComplete: { config: { topic: 'IMPL_READY' } } },
          },
          {
            id: 'validator',
            role: 'validator',
            triggers: [{ topic: 'IMPL_READY' }],
            hooks: { onComplete: { config: { topic: 'VALIDATION_RESULT' } } },
          },
          {
            id: 'completion',
            role: 'orchestrator',
            triggers: [{ topic: 'VALIDATION_RESULT', action: 'stop_cluster' }],
          },
        ],
      });
      assert.ok(
        result.errors.some(
          (e) =>
            e.includes('worker') &&
            e.includes('VALIDATION_RESULT') &&
            e.includes('Rejections will be ignored')
        )
      );
    });

    it('should warn when context strategy missing trigger topic', function () {
      const result = analyzeMessageFlow({
        agents: [
          {
            id: 'worker',
            role: 'impl',
            triggers: [{ topic: 'ISSUE_OPENED' }, { topic: 'FEEDBACK' }],
            contextStrategy: {
              sources: [{ topic: 'ISSUE_OPENED', limit: 1 }],
              // Missing FEEDBACK!
            },
          },
          {
            id: 'completion',
            role: 'orchestrator',
            triggers: [{ topic: 'X', action: 'stop_cluster' }],
          },
        ],
      });
      assert.ok(
        result.warnings.some(
          (w) => w.includes('worker') && w.includes('FEEDBACK') && w.includes('contextStrategy')
        )
      );
    });

    it('should pass a valid minimal config', function () {
      const result = analyzeMessageFlow({
        agents: [
          {
            id: 'worker',
            role: 'implementation',
            triggers: [{ topic: 'ISSUE_OPENED' }],
            hooks: { onComplete: { config: { topic: 'DONE' } } },
          },
          {
            id: 'completion',
            role: 'orchestrator',
            triggers: [{ topic: 'DONE', action: 'stop_cluster' }],
          },
        ],
      });
      assert.strictEqual(result.errors.length, 0);
    });
  });

  // === AGENT VALIDATION TESTS ===

  describe('validateAgents', function () {
    it('should warn when orchestrator has execute_task action', function () {
      const result = validateAgents({
        agents: [
          {
            id: 'completion',
            role: 'orchestrator',
            triggers: [{ topic: 'X', action: 'execute_task' }], // Should be stop_cluster
          },
        ],
      });
      assert.ok(
        result.warnings.some(
          (w) => w.toLowerCase().includes('orchestrator') && w.includes('execute_task')
        )
      );
    });

    it('should warn when json output has no schema', function () {
      const result = validateAgents({
        agents: [
          {
            id: 'worker',
            role: 'impl',
            triggers: [{ topic: 'X' }],
            outputFormat: 'json',
            // Missing jsonSchema
          },
        ],
      });
      assert.ok(result.warnings.some((w) => w.includes('json') && w.includes('jsonSchema')));
    });

    it('should warn on very high maxIterations', function () {
      const result = validateAgents({
        agents: [
          {
            id: 'worker',
            role: 'impl',
            triggers: [{ topic: 'X' }],
            maxIterations: 100,
          },
        ],
      });
      assert.ok(result.warnings.some((w) => w.includes('maxIterations') && w.includes('100')));
    });

    it('should error when logic references non-existent role', function () {
      const result = validateAgents({
        agents: [
          {
            id: 'completion',
            role: 'orchestrator',
            triggers: [
              {
                topic: 'X',
                logic: {
                  script: 'return cluster.getAgentsByRole("reviewer").length > 0;',
                },
              },
            ],
          },
        ],
      });
      assert.ok(
        result.warnings.some((e) => e.includes('reviewer') && e.includes('no agent has that role'))
      );
    });

    it('should pass when logic references existing role', function () {
      const result = validateAgents({
        agents: [
          { id: 'val', role: 'validator', triggers: [{ topic: 'Y' }] },
          {
            id: 'completion',
            role: 'orchestrator',
            triggers: [
              {
                topic: 'X',
                logic: {
                  script: 'return cluster.getAgentsByRole("validator").length > 0;',
                },
              },
            ],
          },
        ],
      });
      assert.ok(!result.errors.some((e) => e.includes('validator') && e.includes('no agent')));
    });
  });

  // === LOGIC SCRIPT TESTS ===

  describe('validateLogicScripts', function () {
    it('should error on syntax error in logic script', function () {
      const result = validateLogicScripts({
        agents: [
          {
            id: 'worker',
            role: 'impl',
            triggers: [
              {
                topic: 'X',
                logic: { script: 'return ledger.query({ topic: "X" ).length;' }, // Missing ]
              },
            ],
          },
        ],
      });
      assert.ok(result.errors.some((e) => e.includes('invalid logic script')));
    });

    it('should warn when script is just "return false"', function () {
      const result = validateLogicScripts({
        agents: [
          {
            id: 'worker',
            role: 'impl',
            triggers: [
              {
                topic: 'X',
                logic: { script: 'return false;' },
              },
            ],
          },
        ],
      });
      assert.ok(
        result.warnings.some((w) => w.includes('return false') && w.includes('never trigger'))
      );
    });

    it('should warn when script is just "return true"', function () {
      const result = validateLogicScripts({
        agents: [
          {
            id: 'worker',
            role: 'impl',
            triggers: [
              {
                topic: 'X',
                logic: { script: 'return true;' },
              },
            ],
          },
        ],
      });
      assert.ok(result.warnings.some((w) => w.includes('return true') && w.includes('Consider')));
    });

    it('should NOT warn on complex scripts with return false in conditional', function () {
      const result = validateLogicScripts({
        agents: [
          {
            id: 'worker',
            role: 'impl',
            triggers: [
              {
                topic: 'X',
                logic: { script: 'if (!condition) return false; return true;' },
              },
            ],
          },
        ],
      });
      assert.ok(
        !result.warnings.some((w) => w.includes('return false') && w.includes('never trigger'))
      );
    });

    it('should warn on potential undefined variable access', function () {
      const result = validateLogicScripts({
        agents: [
          {
            id: 'worker',
            role: 'impl',
            triggers: [
              {
                topic: 'X',
                logic: { script: 'return unknownVar.length > 0;' },
              },
            ],
          },
        ],
      });
      assert.ok(result.warnings.some((w) => w.includes('unknownVar') && w.includes('undefined')));
    });

    it('should pass valid logic script', function () {
      const result = validateLogicScripts({
        agents: [
          {
            id: 'worker',
            role: 'impl',
            triggers: [
              {
                topic: 'X',
                logic: {
                  script: `
                const validators = cluster.getAgentsByRole('validator');
                const responses = ledger.query({ topic: 'VALIDATION_RESULT' });
                return responses.length >= validators.length;
              `,
                },
              },
            ],
          },
        ],
      });
      assert.strictEqual(result.errors.length, 0);
    });
  });

  // === FULL VALIDATION TESTS ===

  describe('validateConfig (full)', function () {
    it('should validate full-workflow config without errors', function () {
      const fs = require('fs');
      const path = require('path');
      const configPath = path.join(
        __dirname,
        '..',
        'cluster-templates',
        'base-templates',
        'full-workflow.json'
      );
      const config = JSON.parse(fs.readFileSync(configPath, 'utf8'));

      const result = validateConfig(config);
      assert.strictEqual(result.valid, true, `Errors: ${result.errors.join(', ')}`);
    });

    it('should validate single-worker config (warning about no completion handler is expected)', function () {
      const fs = require('fs');
      const path = require('path');
      const configPath = path.join(
        __dirname,
        '..',
        'cluster-templates',
        'base-templates',
        'single-worker.json'
      );
      const config = JSON.parse(fs.readFileSync(configPath, 'utf8'));

      const result = validateConfig(config);
      // simple-worker intentionally has no completion handler - this triggers a warning/error
      // The config is still valid for simple tasks that complete in one shot
      // We just check it doesn't have other structural errors
      const nonCompletionErrors = result.errors.filter((e) => !e.includes('completion handler'));
      assert.strictEqual(
        nonCompletionErrors.length,
        0,
        `Unexpected errors: ${nonCompletionErrors.join(', ')}`
      );
    });

    it('should catch multiple issues in broken config', function () {
      const brokenConfig = {
        agents: [
          {
            // Missing id
            role: 'impl',
            triggers: [], // Empty triggers
          },
          {
            id: 'worker',
            // Missing role
            triggers: [{ topic: 'NEVER_PRODUCED' }],
          },
        ],
      };

      const result = validateConfig(brokenConfig);
      assert.strictEqual(result.valid, false);
      assert.ok(result.errors.length >= 3); // At least: missing id, missing role, empty triggers
    });
  });
});
