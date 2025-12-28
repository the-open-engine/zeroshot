/**
 * CRITICAL TEST: Verify fixer agent receives aggressive instructions about:
 * 1. FORBIDDEN approaches (shortcuts that hide problems)
 * 2. How to PROCESS rejection feedback
 *
 * This prevents fixers from "fixing" by disabling/suppressing instead of actually fixing.
 */

const assert = require('assert');
const AgentWrapper = require('../src/agent-wrapper');
const MessageBus = require('../src/message-bus');
const Ledger = require('../src/ledger');
const path = require('path');
const fs = require('fs');
const os = require('os');

describe('Fixer Instructions - CRITICAL', () => {
  let tempDir;
  let ledger;
  let messageBus;

  beforeEach(() => {
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-fixer-test-'));
    const dbPath = path.join(tempDir, 'test-ledger.db');
    ledger = new Ledger(dbPath);
    messageBus = new MessageBus(ledger);
  });

  afterEach(() => {
    if (tempDir && fs.existsSync(tempDir)) {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  describe('FORBIDDEN approaches section', () => {
    it('must exist in fixer prompt', () => {
      const templatePath = path.join(
        __dirname,
        '..',
        'cluster-templates',
        'base-templates',
        'debug-workflow.json'
      );
      const template = JSON.parse(fs.readFileSync(templatePath, 'utf8'));
      const fixerConfig = template.agents.find((a) => a.id === 'fixer');

      assert(fixerConfig, 'Fixer agent must exist');

      const prompt = fixerConfig.prompt.system;

      assert(prompt.includes('FORBIDDEN'), 'Prompt must have FORBIDDEN section');
      assert(
        prompt.includes('SHORTCUTS') || prompt.includes('HIDE'),
        'Must explain WHY these are forbidden'
      );
    });

    it('must forbid suppressing/disabling errors', () => {
      const templatePath = path.join(
        __dirname,
        '..',
        'cluster-templates',
        'base-templates',
        'debug-workflow.json'
      );
      const template = JSON.parse(fs.readFileSync(templatePath, 'utf8'));
      const fixerConfig = template.agents.find((a) => a.id === 'fixer');
      const prompt = fixerConfig.prompt.system;

      // Generic - not tool-specific
      assert(
        prompt.includes('disable') || prompt.includes('suppress'),
        'Must forbid disabling/suppressing errors'
      );
    });

    it('must forbid changing test expectations to match broken behavior', () => {
      const templatePath = path.join(
        __dirname,
        '..',
        'cluster-templates',
        'base-templates',
        'debug-workflow.json'
      );
      const template = JSON.parse(fs.readFileSync(templatePath, 'utf8'));
      const fixerConfig = template.agents.find((a) => a.id === 'fixer');
      const prompt = fixerConfig.prompt.system;

      assert(
        prompt.includes('test expectations') || prompt.includes('broken behavior'),
        'Must forbid changing tests to match bugs'
      );
    });

    it('must state that hidden problems are NOT fixed', () => {
      const templatePath = path.join(
        __dirname,
        '..',
        'cluster-templates',
        'base-templates',
        'debug-workflow.json'
      );
      const template = JSON.parse(fs.readFileSync(templatePath, 'utf8'));
      const fixerConfig = template.agents.find((a) => a.id === 'fixer');
      const prompt = fixerConfig.prompt.system;

      assert(
        prompt.includes('HIDDEN') || prompt.includes('NOT FIXED'),
        'Must state that hiding problems is not fixing them'
      );
    });
  });

  describe('AGGRESSIVE rejection handling', () => {
    it('must use AGGRESSIVE language - no polite bullshit', () => {
      const templatePath = path.join(
        __dirname,
        '..',
        'cluster-templates',
        'base-templates',
        'debug-workflow.json'
      );
      const template = JSON.parse(fs.readFileSync(templatePath, 'utf8'));
      const fixerConfig = template.agents.find((a) => a.id === 'fixer');
      const prompt = fixerConfig.prompt.system;

      // Must have aggressive tone - not polite suggestions
      const hasAggressiveLanguage =
        prompt.includes('FUCKING') ||
        prompt.includes('DO NOT') ||
        prompt.includes('NEVER') ||
        prompt.includes('STOP') ||
        prompt.includes('WRONG');

      assert(hasAggressiveLanguage, 'Prompt must use AGGRESSIVE language, not polite suggestions');
    });

    it('must explicitly tell agent to STOP and READ on rejection', () => {
      const templatePath = path.join(
        __dirname,
        '..',
        'cluster-templates',
        'base-templates',
        'debug-workflow.json'
      );
      const template = JSON.parse(fs.readFileSync(templatePath, 'utf8'));
      const fixerConfig = template.agents.find((a) => a.id === 'fixer');
      const prompt = fixerConfig.prompt.system;

      assert(
        prompt.includes('STOP') || prompt.includes('READ'),
        'Must tell agent to STOP and READ feedback before retrying'
      );
    });

    it('must instruct fixer to READ feedback', () => {
      const templatePath = path.join(
        __dirname,
        '..',
        'cluster-templates',
        'base-templates',
        'debug-workflow.json'
      );
      const template = JSON.parse(fs.readFileSync(templatePath, 'utf8'));
      const fixerConfig = template.agents.find((a) => a.id === 'fixer');
      const prompt = fixerConfig.prompt.system;

      assert(
        prompt.includes('READ') && prompt.includes('FEEDBACK'),
        'Must instruct fixer to READ FEEDBACK'
      );
    });

    it('must warn against repeating failed approaches', () => {
      const templatePath = path.join(
        __dirname,
        '..',
        'cluster-templates',
        'base-templates',
        'debug-workflow.json'
      );
      const template = JSON.parse(fs.readFileSync(templatePath, 'utf8'));
      const fixerConfig = template.agents.find((a) => a.id === 'fixer');
      const prompt = fixerConfig.prompt.system;

      assert(
        prompt.includes('repeat') || prompt.includes('same approach') || prompt.includes('blindly'),
        'Must warn against repeating failed approaches'
      );
    });

    it('must tell fixer to try DIFFERENT approach on failure', () => {
      const templatePath = path.join(
        __dirname,
        '..',
        'cluster-templates',
        'base-templates',
        'debug-workflow.json'
      );
      const template = JSON.parse(fs.readFileSync(templatePath, 'utf8'));
      const fixerConfig = template.agents.find((a) => a.id === 'fixer');
      const prompt = fixerConfig.prompt.system;

      assert(
        prompt.includes('DIFFERENT') || prompt.includes('rethink'),
        'Must instruct trying different approach on failure'
      );
    });
  });

  describe('Context injection on rejection', () => {
    it('must inject VALIDATION_RESULT content into fixer context on retry', () => {
      const clusterId = 'test-rejection-context';
      const clusterCreatedAt = Date.now() - 60000;

      // Simulate investigation
      messageBus.publish({
        cluster_id: clusterId,
        topic: 'INVESTIGATION_COMPLETE',
        sender: 'investigator',
        content: {
          text: 'Found bug',
          data: { rootCause: 'test', evidence: [] },
        },
      });

      // Simulate rejection with detailed feedback
      messageBus.publish({
        cluster_id: clusterId,
        topic: 'VALIDATION_RESULT',
        sender: 'tester',
        content: {
          text: 'Your fix is wrong because XYZ',
          data: {
            approved: false,
            errors: ['Error A still exists', 'Error B was introduced'],
            suggestion: 'Try approach ABC instead',
          },
        },
      });

      const fixerConfig = {
        id: 'fixer',
        role: 'implementation',
        timeout: 0,
        contextStrategy: {
          sources: [
            { topic: 'INVESTIGATION_COMPLETE', limit: 1 },
            { topic: 'VALIDATION_RESULT', since: 'last_task_end', limit: 5 },
          ],
        },
      };

      const mockCluster = {
        id: clusterId,
        createdAt: clusterCreatedAt,
        agents: [],
      };

      const fixer = new AgentWrapper(fixerConfig, messageBus, mockCluster, {
        testMode: true,
        mockSpawnFn: () => {},
      });

      const context = fixer._buildContext({
        cluster_id: clusterId,
        topic: 'VALIDATION_RESULT',
        sender: 'tester',
        content: { text: 'rejection trigger' },
      });

      // Verify rejection details are in context
      assert(context.includes('VALIDATION_RESULT'), 'Context must include VALIDATION_RESULT topic');
      assert(context.includes('Your fix is wrong'), 'Context must include rejection message');
      assert(
        context.includes('approved') || context.includes('false'),
        'Context must include approval status'
      );
      assert(
        context.includes('Error A') || context.includes('Error B'),
        'Context must include specific errors from rejection'
      );

      console.log('✅ Rejection feedback is fully injected into fixer context on retry');
    });

    it('must include ALL validation results when multiple rejections occur', () => {
      const clusterId = 'test-multi-rejection';
      const clusterCreatedAt = Date.now() - 120000;

      // First rejection
      messageBus.publish({
        cluster_id: clusterId,
        topic: 'VALIDATION_RESULT',
        sender: 'tester',
        content: {
          text: 'First rejection: Problem X',
          data: { approved: false, errors: ['Problem X'] },
        },
      });

      // Second rejection (after retry)
      messageBus.publish({
        cluster_id: clusterId,
        topic: 'VALIDATION_RESULT',
        sender: 'tester',
        content: {
          text: 'Second rejection: Problem X still exists, now also Problem Y',
          data: { approved: false, errors: ['Problem X', 'Problem Y'] },
        },
      });

      const fixerConfig = {
        id: 'fixer',
        timeout: 0,
        contextStrategy: {
          sources: [{ topic: 'VALIDATION_RESULT', since: 'last_task_end', limit: 10 }],
        },
      };

      const mockCluster = {
        id: clusterId,
        createdAt: clusterCreatedAt,
        agents: [],
      };

      const fixer = new AgentWrapper(fixerConfig, messageBus, mockCluster, {
        testMode: true,
        mockSpawnFn: () => {},
      });

      const context = fixer._buildContext({
        cluster_id: clusterId,
        topic: 'VALIDATION_RESULT',
        sender: 'tester',
        content: { text: 'trigger' },
      });

      // Both rejections should be visible
      assert(
        context.includes('First rejection') || context.includes('Problem X'),
        'Context must include first rejection'
      );
      assert(
        context.includes('Second rejection') || context.includes('Problem Y'),
        'Context must include second rejection'
      );

      console.log('✅ Multiple rejections are ALL included in fixer context');
    });
  });
});
