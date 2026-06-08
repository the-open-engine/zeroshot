/**
 * Integration tests for LogicEngine trigger evaluation
 *
 * Tests trigger scripts with real ledger data, helper functions,
 * and timeout/error handling.
 */

const assert = require('node:assert');
const path = require('path');
const fs = require('fs');
const os = require('os');

const LogicEngine = require('../../src/logic-engine');
const MessageBus = require('../../src/message-bus');
const Ledger = require('../../src/ledger');
const {
  generateGitPusherAgent,
  SHARED_TRIGGER_SCRIPT,
} = require('../../src/agents/git-pusher-template');

let tempDir;
let ledger;
let messageBus;
let logicEngine;
let cluster;

describe('Trigger Evaluation Integration', function () {
  this.timeout(10000);

  beforeEach(() => {
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-trigger-test-'));
    const dbPath = path.join(tempDir, 'test.db');
    ledger = new Ledger(dbPath);
    messageBus = new MessageBus(ledger);

    cluster = {
      id: 'test-cluster-1',
      agents: [
        { id: 'worker', role: 'implementation' },
        { id: 'validator-1', role: 'validator' },
        { id: 'validator-2', role: 'validator' },
        { id: 'reviewer', role: 'reviewer' },
      ],
      getAgent: function (id) {
        return this.agents.find((a) => a.id === id);
      },
      getAgentsByRole: function (role) {
        return this.agents.filter((a) => a.role === role);
      },
    };

    logicEngine = new LogicEngine(messageBus, cluster);
  });

  afterEach(() => {
    if (ledger) ledger.close();
    if (tempDir && fs.existsSync(tempDir)) {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  defineBasicLedgerQueryTests();
  defineClusterApiTests();
  defineHelperFunctionTests();
  defineErrorHandlingTests();
  defineScriptValidationTests();
  defineComplexConsensusTests();
  defineGitPusherTriggerTests();
});

function defineBasicLedgerQueryTests() {
  describe('Basic Ledger Queries', () => {
    it('should evaluate ledger.findLast()', () => {
      messageBus.publish({
        cluster_id: cluster.id,
        topic: 'TEST_TOPIC',
        sender: 'agent-1',
        content: { text: 'First' },
      });

      messageBus.publish({
        cluster_id: cluster.id,
        topic: 'TEST_TOPIC',
        sender: 'agent-2',
        content: { text: 'Second' },
      });

      const script = `
        const last = ledger.findLast({ topic: 'TEST_TOPIC' });
        return last && last.sender === 'agent-2';
      `;

      const result = logicEngine.evaluate(
        script,
        {
          id: 'evaluator',
          cluster_id: cluster.id,
        },
        { topic: 'TRIGGER' }
      );

      assert.strictEqual(result, true);
    });

    it('should evaluate ledger.query() with filters', () => {
      messageBus.publish({
        cluster_id: cluster.id,
        topic: 'VALIDATION_RESULT',
        sender: 'validator-1',
        content: { data: { approved: true } },
      });

      messageBus.publish({
        cluster_id: cluster.id,
        topic: 'OTHER_TOPIC',
        sender: 'other',
      });

      messageBus.publish({
        cluster_id: cluster.id,
        topic: 'VALIDATION_RESULT',
        sender: 'validator-2',
        content: { data: { approved: false } },
      });

      const script = `
        const results = ledger.query({ topic: 'VALIDATION_RESULT' });
        return results.length === 2;
      `;

      const result = logicEngine.evaluate(
        script,
        {
          id: 'evaluator',
          cluster_id: cluster.id,
        },
        { topic: 'TRIGGER' }
      );

      assert.strictEqual(result, true);
    });

    it('should evaluate ledger.count()', () => {
      for (let i = 0; i < 5; i++) {
        messageBus.publish({
          cluster_id: cluster.id,
          topic: 'COUNTED_TOPIC',
          sender: `agent-${i}`,
        });
      }

      const script = `
        return ledger.count({ topic: 'COUNTED_TOPIC' }) === 5;
      `;

      const result = logicEngine.evaluate(
        script,
        {
          id: 'evaluator',
          cluster_id: cluster.id,
        },
        { topic: 'TRIGGER' }
      );

      assert.strictEqual(result, true);
    });
  });
}

function defineClusterApiTests() {
  describe('Cluster API', () => {
    it('should evaluate cluster.getAgentsByRole()', () => {
      const script = `
        const validators = cluster.getAgentsByRole('validator');
        return validators.length === 2;
      `;

      const result = logicEngine.evaluate(
        script,
        {
          id: 'evaluator',
          cluster_id: cluster.id,
        },
        { topic: 'TRIGGER' }
      );

      assert.strictEqual(result, true);
    });

    it('should evaluate cluster.getAgent()', () => {
      const script = `
        const worker = cluster.getAgent('worker');
        return worker && worker.role === 'implementation';
      `;

      const result = logicEngine.evaluate(
        script,
        {
          id: 'evaluator',
          cluster_id: cluster.id,
        },
        { topic: 'TRIGGER' }
      );

      assert.strictEqual(result, true);
    });
  });
}

function defineHelperFunctionTests() {
  describe('Helper Functions', () => {
    it('helpers.allResponded() should check all validators responded', () => {
      // Publish implementation ready first
      messageBus.publish({
        cluster_id: cluster.id,
        topic: 'IMPLEMENTATION_READY',
        sender: 'worker',
        timestamp: Date.now(),
      });

      const implTime = Date.now();

      // Both validators respond
      messageBus.publish({
        cluster_id: cluster.id,
        topic: 'VALIDATION_RESULT',
        sender: 'validator-1',
        timestamp: implTime + 100,
      });

      messageBus.publish({
        cluster_id: cluster.id,
        topic: 'VALIDATION_RESULT',
        sender: 'validator-2',
        timestamp: implTime + 200,
      });

      const script = `
        const validators = cluster.getAgentsByRole('validator');
        const lastImpl = ledger.findLast({ topic: 'IMPLEMENTATION_READY' });
        return helpers.allResponded(validators, 'VALIDATION_RESULT', lastImpl.timestamp);
      `;

      const result = logicEngine.evaluate(
        script,
        {
          id: 'evaluator',
          cluster_id: cluster.id,
        },
        { topic: 'TRIGGER' }
      );

      assert.strictEqual(result, true);
    });

    it('helpers.allResponded() should return false when some validators missing', () => {
      messageBus.publish({
        cluster_id: cluster.id,
        topic: 'IMPLEMENTATION_READY',
        sender: 'worker',
      });

      // Only one validator responds
      messageBus.publish({
        cluster_id: cluster.id,
        topic: 'VALIDATION_RESULT',
        sender: 'validator-1',
      });

      const script = `
        const validators = cluster.getAgentsByRole('validator');
        const lastImpl = ledger.findLast({ topic: 'IMPLEMENTATION_READY' });
        return helpers.allResponded(validators, 'VALIDATION_RESULT', lastImpl.timestamp);
      `;

      const result = logicEngine.evaluate(
        script,
        {
          id: 'evaluator',
          cluster_id: cluster.id,
        },
        { topic: 'TRIGGER' }
      );

      assert.strictEqual(result, false);
    });

    it('helpers.hasConsensus() should check all approved', () => {
      messageBus.publish({
        cluster_id: cluster.id,
        topic: 'VALIDATION_RESULT',
        sender: 'validator-1',
        content: { data: { approved: true } },
      });

      messageBus.publish({
        cluster_id: cluster.id,
        topic: 'VALIDATION_RESULT',
        sender: 'validator-2',
        content: { data: { approved: true } },
      });

      const script = `
        return helpers.hasConsensus('VALIDATION_RESULT', 0);
      `;

      const result = logicEngine.evaluate(
        script,
        {
          id: 'evaluator',
          cluster_id: cluster.id,
        },
        { topic: 'TRIGGER' }
      );

      assert.strictEqual(result, true);
    });

    it('helpers.hasConsensus() should return false on rejection', () => {
      messageBus.publish({
        cluster_id: cluster.id,
        topic: 'VALIDATION_RESULT',
        sender: 'validator-1',
        content: { data: { approved: true } },
      });

      messageBus.publish({
        cluster_id: cluster.id,
        topic: 'VALIDATION_RESULT',
        sender: 'validator-2',
        content: { data: { approved: false } },
      });

      const script = `
        return helpers.hasConsensus('VALIDATION_RESULT', 0);
      `;

      const result = logicEngine.evaluate(
        script,
        {
          id: 'evaluator',
          cluster_id: cluster.id,
        },
        { topic: 'TRIGGER' }
      );

      assert.strictEqual(result, false);
    });
  });
}

function defineErrorHandlingTests() {
  describe('Error Handling', () => {
    it('should return false on script error (fail-safe)', () => {
      const script = `
        throw new Error('Intentional error');
      `;

      const result = logicEngine.evaluate(
        script,
        {
          id: 'evaluator',
          cluster_id: cluster.id,
        },
        { topic: 'TRIGGER' }
      );

      assert.strictEqual(result, false);
    });

    it('should return false on undefined variable access', () => {
      const script = `
        return undefinedVariable.property;
      `;

      const result = logicEngine.evaluate(
        script,
        {
          id: 'evaluator',
          cluster_id: cluster.id,
        },
        { topic: 'TRIGGER' }
      );

      assert.strictEqual(result, false);
    });

    it('should handle null ledger results gracefully', () => {
      const script = `
        const notFound = ledger.findLast({ topic: 'NONEXISTENT' });
        return notFound === null;
      `;

      const result = logicEngine.evaluate(
        script,
        {
          id: 'evaluator',
          cluster_id: cluster.id,
        },
        { topic: 'TRIGGER' }
      );

      assert.strictEqual(result, true);
    });
  });
}

function defineScriptValidationTests() {
  describe('Script Validation', () => {
    it('should validate script syntax', () => {
      const validScript = 'return true;';
      const invalidScript = 'return {{{';

      const validResult = logicEngine.validateScript(validScript);
      const invalidResult = logicEngine.validateScript(invalidScript);

      assert.strictEqual(validResult.valid, true);
      assert.strictEqual(invalidResult.valid, false);
    });
  });
}

function defineComplexConsensusTests() {
  describe('Complex Consensus Logic', () => {
    it('should evaluate complete validator consensus pattern', () => {
      // Worker publishes implementation
      messageBus.publish({
        cluster_id: cluster.id,
        topic: 'IMPLEMENTATION_READY',
        sender: 'worker',
        timestamp: Date.now(),
      });

      const implTime = Date.now();

      // All validators approve
      messageBus.publish({
        cluster_id: cluster.id,
        topic: 'VALIDATION_RESULT',
        sender: 'validator-1',
        timestamp: implTime + 100,
        content: { data: { approved: true } },
      });

      messageBus.publish({
        cluster_id: cluster.id,
        topic: 'VALIDATION_RESULT',
        sender: 'validator-2',
        timestamp: implTime + 200,
        content: { data: { approved: true } },
      });

      // Real-world consensus script
      const script = `
        const validators = cluster.getAgentsByRole('validator');
        const lastImpl = ledger.findLast({ topic: 'IMPLEMENTATION_READY' });
        if (!lastImpl) return false;

        const results = ledger.query({
          topic: 'VALIDATION_RESULT',
          since: lastImpl.timestamp
        });

        // Need responses from all validators
        if (results.length < validators.length) return false;

        // All must approve
        return results.every(r => r.content?.data?.approved === true);
      `;

      const result = logicEngine.evaluate(
        script,
        {
          id: 'completion-detector',
          cluster_id: cluster.id,
        },
        { topic: 'VALIDATION_RESULT' }
      );

      assert.strictEqual(result, true);
    });
  });
}

function createQualityGate(overrides = {}) {
  const id = overrides.id || 'repo-quality';
  const scope = overrides.scope || 'repo';
  const status = overrides.status || 'PASS';
  const exitCode = Object.prototype.hasOwnProperty.call(overrides, 'exitCode')
    ? overrides.exitCode
    : 0;
  const gate = {
    id,
    status,
    scope,
    completedAt: Date.now(),
    evidence: {
      command: overrides.command || `quality-check --scope ${scope}`,
      exitCode,
      output: overrides.output || `${id} passed`,
    },
  };
  if (Object.prototype.hasOwnProperty.call(overrides, 'completedAt')) {
    gate.completedAt = overrides.completedAt;
  }
  if (Object.prototype.hasOwnProperty.call(overrides, 'timestamp')) {
    gate.timestamp = overrides.timestamp;
  }
  if (overrides.stale === true) {
    gate.stale = true;
  }
  if (overrides.reason) {
    gate.reason = overrides.reason;
  }
  return gate;
}

function publishImplementationReady(timestamp = Date.now()) {
  return messageBus.publish({
    cluster_id: cluster.id,
    topic: 'IMPLEMENTATION_READY',
    sender: 'worker',
    timestamp,
  });
}

function publishValidationResult({
  sender,
  timestamp,
  approved = true,
  criteriaResults,
  qualityGates,
  errors = [],
  stage,
}) {
  const data = { approved, errors };
  if (criteriaResults !== undefined) data.criteriaResults = criteriaResults;
  if (qualityGates !== undefined) data.qualityGates = qualityGates;
  if (stage !== undefined) data.stage = stage;
  return messageBus.publish({
    cluster_id: cluster.id,
    topic: 'VALIDATION_RESULT',
    sender,
    timestamp,
    content: { data },
  });
}

function evaluateGitPusherHandoff(options = {}) {
  const requiredQualityGates = Object.prototype.hasOwnProperty.call(options, 'requiredQualityGates')
    ? options.requiredQualityGates
    : [{ id: 'repo-quality' }];
  const shouldExecute = logicEngine.evaluate(
    SHARED_TRIGGER_SCRIPT,
    { id: 'git-pusher', cluster_id: cluster.id, requiredQualityGates },
    { topic: 'VALIDATION_RESULT' }
  );
  const mutationCommands = [];
  if (shouldExecute) {
    mutationCommands.push('git commit', 'git push', 'gh pr merge');
  }
  return { shouldExecute, mutationCommands };
}

function evaluateGeneratedGitPusherHandoff(gitPusherConfig) {
  const withoutExistingPusher = cluster.agents.filter((candidate) => candidate.id !== 'git-pusher');
  cluster.agents = [...withoutExistingPusher, gitPusherConfig];
  const shouldExecute = logicEngine.evaluate(
    SHARED_TRIGGER_SCRIPT,
    { id: gitPusherConfig.id, cluster_id: cluster.id },
    { topic: 'VALIDATION_RESULT' }
  );
  const mutationCommands = [];
  if (shouldExecute) {
    mutationCommands.push('git commit', 'git push', 'gh pr merge');
  }
  return { shouldExecute, mutationCommands };
}

function captureConsoleError(fn) {
  const originalError = console.error;
  const messages = [];
  console.error = (...args) => {
    messages.push(args.map((arg) => String(arg)).join(' '));
  };
  try {
    return { result: fn(), messages };
  } finally {
    console.error = originalError;
  }
}

function defineGitPusherTriggerTests() {
  describe('Git-pusher Trigger Evidence', () => {
    it('allows pusher handoff when configured quality gate passes', () => {
      const impl = publishImplementationReady();

      publishValidationResult({
        sender: 'validator-1',
        timestamp: impl.timestamp + 100,
        qualityGates: [createQualityGate({ scope: 'repo' })],
        criteriaResults: [
          {
            id: 'AC1',
            status: 'PASS',
            evidence: { command: 'npm test', exitCode: 0, output: '' },
          },
          {
            id: 'AC2',
            status: 'CANNOT_VALIDATE',
            reason: 'Docker not available',
          },
        ],
      });

      publishValidationResult({
        sender: 'validator-2',
        timestamp: impl.timestamp + 200,
      });

      const result = evaluateGitPusherHandoff();

      assert.strictEqual(result.shouldExecute, true);
      assert.deepStrictEqual(result.mutationCommands, ['git commit', 'git push', 'gh pr merge']);
    });

    it('should accept consensus-only VALIDATION_RESULT when validators do not publish directly', () => {
      // Simulate staged validation (quick/heavy): validators publish stage-specific topics,
      // and only a coordinator publishes a single consolidated VALIDATION_RESULT.
      cluster.agents.push(
        { id: 'validator-3', role: 'validator' },
        { id: 'validator-4', role: 'validator' }
      );

      const impl = publishImplementationReady();

      publishValidationResult({
        sender: 'consensus-coordinator',
        timestamp: impl.timestamp + 100,
        stage: 'heavy',
        qualityGates: [createQualityGate({ scope: 'repo' })],
      });

      const result = evaluateGitPusherHandoff();

      assert.strictEqual(result.shouldExecute, true);
      assert.deepStrictEqual(result.mutationCommands, ['git commit', 'git push', 'gh pr merge']);
    });

    it('should not accept consensus-only VALIDATION_RESULT when rejected', () => {
      cluster.agents.push(
        { id: 'validator-3', role: 'validator' },
        { id: 'validator-4', role: 'validator' }
      );

      const impl = publishImplementationReady();

      publishValidationResult({
        sender: 'consensus-coordinator',
        timestamp: impl.timestamp + 100,
        approved: false,
        stage: 'quick',
      });

      const result = evaluateGitPusherHandoff();

      assert.strictEqual(result.shouldExecute, false);
      assert.deepStrictEqual(result.mutationCommands, []);
    });

    it('does not require a quality gate when none are configured', () => {
      const impl = publishImplementationReady();

      publishValidationResult({
        sender: 'validator-1',
        timestamp: impl.timestamp + 100,
      });
      publishValidationResult({
        sender: 'validator-2',
        timestamp: impl.timestamp + 200,
      });

      const result = evaluateGitPusherHandoff({ requiredQualityGates: [] });

      assert.strictEqual(result.shouldExecute, true);
      assert.deepStrictEqual(result.mutationCommands, ['git commit', 'git push', 'gh pr merge']);
    });

    it('blocks pusher mutation when configured quality gate is missing', () => {
      const impl = publishImplementationReady();

      publishValidationResult({
        sender: 'validator-1',
        timestamp: impl.timestamp + 100,
        criteriaResults: [
          {
            id: 'AC1',
            status: 'PASS',
            evidence: { command: 'npm test', exitCode: 0, output: 'pass' },
          },
        ],
      });
      publishValidationResult({
        sender: 'validator-2',
        timestamp: impl.timestamp + 200,
      });

      const { result, messages } = captureConsoleError(evaluateGitPusherHandoff);

      assert.strictEqual(result.shouldExecute, false);
      assert.deepStrictEqual(result.mutationCommands, []);
      assert.match(messages.join('\n'), /Required quality gate missing/);
    });

    it('blocks pusher mutation when generated pusher config requires a missing gate', () => {
      const impl = publishImplementationReady();
      const gitPusherConfig = generateGitPusherAgent('github', {
        requiredQualityGates: [{ id: 'repo-quality', scope: 'repo' }],
      });

      publishValidationResult({
        sender: 'validator-1',
        timestamp: impl.timestamp + 100,
      });
      publishValidationResult({
        sender: 'validator-2',
        timestamp: impl.timestamp + 200,
      });

      const { result, messages } = captureConsoleError(() =>
        evaluateGeneratedGitPusherHandoff(gitPusherConfig)
      );

      assert.strictEqual(result.shouldExecute, false);
      assert.deepStrictEqual(result.mutationCommands, []);
      assert.match(messages.join('\n'), /Required quality gate missing/);
    });

    it('blocks pusher mutation when generated pusher config requires a failing gate', () => {
      const impl = publishImplementationReady();
      const gitPusherConfig = generateGitPusherAgent('github', {
        requiredQualityGates: [{ id: 'repo-quality', scope: 'repo' }],
      });

      publishValidationResult({
        sender: 'validator-1',
        timestamp: impl.timestamp + 100,
        qualityGates: [
          createQualityGate({
            scope: 'repo',
            status: 'FAIL',
            exitCode: 1,
            output: 'configured quality gate failed',
          }),
        ],
      });
      publishValidationResult({
        sender: 'validator-2',
        timestamp: impl.timestamp + 200,
      });

      const { result, messages } = captureConsoleError(() =>
        evaluateGeneratedGitPusherHandoff(gitPusherConfig)
      );

      assert.strictEqual(result.shouldExecute, false);
      assert.deepStrictEqual(result.mutationCommands, []);
      const log = messages.join('\n');
      assert.match(log, /Required quality gate blocked/);
      assert.match(log, /configured quality gate failed/);
    });

    it('blocks pusher mutation when frontend quality gate fails', () => {
      const impl = publishImplementationReady();

      publishValidationResult({
        sender: 'validator-1',
        timestamp: impl.timestamp + 100,
        qualityGates: [
          createQualityGate({
            scope: 'frontend',
            status: 'FAIL',
            exitCode: 1,
            output: 'type check failed',
          }),
        ],
      });
      publishValidationResult({
        sender: 'validator-2',
        timestamp: impl.timestamp + 200,
      });

      const { result, messages } = captureConsoleError(evaluateGitPusherHandoff);

      assert.strictEqual(result.shouldExecute, false);
      assert.deepStrictEqual(result.mutationCommands, []);
      const log = messages.join('\n');
      assert.match(log, /Required quality gate blocked/);
      assert.match(log, /scope=frontend/);
      assert.match(log, /type check failed/);
    });

    it('blocks pusher mutation when workspace manifest quality gate fails', () => {
      const impl = publishImplementationReady();

      publishValidationResult({
        sender: 'validator-1',
        timestamp: impl.timestamp + 100,
        qualityGates: [
          createQualityGate({
            scope: 'workspace-manifest',
            status: 'FAIL',
            exitCode: 1,
            output: 'workspace manifest validation failed',
          }),
        ],
      });
      publishValidationResult({
        sender: 'validator-2',
        timestamp: impl.timestamp + 200,
      });

      const { result, messages } = captureConsoleError(evaluateGitPusherHandoff);

      assert.strictEqual(result.shouldExecute, false);
      assert.deepStrictEqual(result.mutationCommands, []);
      const log = messages.join('\n');
      assert.match(log, /Required quality gate blocked/);
      assert.match(log, /scope=workspace-manifest/);
      assert.match(log, /workspace manifest validation failed/);
    });

    it('blocks pusher mutation when configured quality gate is unavailable', () => {
      const impl = publishImplementationReady();

      publishValidationResult({
        sender: 'validator-1',
        timestamp: impl.timestamp + 100,
        qualityGates: [
          createQualityGate({
            scope: 'repo',
            status: 'UNAVAILABLE',
            exitCode: 127,
            output: 'quality tool unavailable',
          }),
        ],
      });
      publishValidationResult({
        sender: 'validator-2',
        timestamp: impl.timestamp + 200,
      });

      const { result, messages } = captureConsoleError(evaluateGitPusherHandoff);

      assert.strictEqual(result.shouldExecute, false);
      assert.deepStrictEqual(result.mutationCommands, []);
      const log = messages.join('\n');
      assert.match(log, /Required quality gate blocked/);
      assert.match(log, /quality tool unavailable/);
    });

    it('blocks pusher mutation when configured quality gate is stale', () => {
      const impl = publishImplementationReady();

      publishValidationResult({
        sender: 'validator-1',
        timestamp: impl.timestamp + 100,
        qualityGates: [
          createQualityGate({
            scope: 'repo',
            status: 'PASS',
            exitCode: 0,
            completedAt: impl.timestamp - 1,
            output: 'old pass',
          }),
        ],
      });
      publishValidationResult({
        sender: 'validator-2',
        timestamp: impl.timestamp + 200,
      });

      const { result, messages } = captureConsoleError(evaluateGitPusherHandoff);

      assert.strictEqual(result.shouldExecute, false);
      assert.deepStrictEqual(result.mutationCommands, []);
      assert.match(messages.join('\n'), /Required quality gate blocked/);
    });

    it('blocks pusher mutation when configured quality gate has no evidence timestamp', () => {
      const impl = publishImplementationReady();
      const gate = createQualityGate({ scope: 'repo' });
      delete gate.completedAt;

      publishValidationResult({
        sender: 'validator-1',
        timestamp: impl.timestamp + 100,
        qualityGates: [gate],
      });
      publishValidationResult({
        sender: 'validator-2',
        timestamp: impl.timestamp + 200,
      });

      const { result, messages } = captureConsoleError(evaluateGitPusherHandoff);

      assert.strictEqual(result.shouldExecute, false);
      assert.deepStrictEqual(result.mutationCommands, []);
      assert.match(messages.join('\n'), /missing completedAt/);
    });

    it('blocks pusher mutation when configured status gate fails', () => {
      const impl = publishImplementationReady();

      publishValidationResult({
        sender: 'validator-1',
        timestamp: impl.timestamp + 100,
        qualityGates: [
          createQualityGate({ id: 'repo-quality', scope: 'repo' }),
          createQualityGate({
            id: 'ci-status',
            scope: 'merge',
            status: 'FAIL',
            exitCode: 1,
            output: 'required status check failed',
          }),
        ],
      });
      publishValidationResult({
        sender: 'validator-2',
        timestamp: impl.timestamp + 200,
      });

      const { result, messages } = captureConsoleError(() =>
        evaluateGitPusherHandoff({
          requiredQualityGates: [{ id: 'repo-quality' }, { id: 'ci-status' }],
        })
      );

      assert.strictEqual(result.shouldExecute, false);
      assert.deepStrictEqual(result.mutationCommands, []);
      const log = messages.join('\n');
      assert.match(log, /Required quality gate blocked/);
      assert.match(log, /gate=ci-status/);
      assert.match(log, /required status check failed/);
    });
  });
}
