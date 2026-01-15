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
