/**
 * CRITICAL TEST: Verify rejection content is FULLY injected into worker context
 *
 * This test ensures that when a validator rejects with detailed issues,
 * the worker agent's context contains the COMPLETE rejection details.
 */

const assert = require('assert');
const AgentWrapper = require('../src/agent-wrapper');
const MessageBus = require('../src/message-bus');
const Ledger = require('../src/ledger');
const path = require('path');
const fs = require('fs');
const os = require('os');

let tempDir;
let ledger;
let messageBus;

describe('Context Injection - CRITICAL', () => {
  beforeEach(() => {
    // Create temp directory for test ledger
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-test-'));
    const dbPath = path.join(tempDir, 'test-ledger.db');

    ledger = new Ledger(dbPath);
    messageBus = new MessageBus(ledger);
  });

  afterEach(() => {
    // Cleanup temp directory
    if (tempDir && fs.existsSync(tempDir)) {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  registerFullRejectionContextTest();
  registerMultipleRejectionsContextTest();
});

function registerFullRejectionContextTest() {
  it('should inject FULL rejection details into worker context', () => {
    const clusterId = 'test-cluster-123';
    const clusterCreatedAt = Date.now() - 60000;

    // Simulate validator publishing a detailed rejection
    const detailedIssues = [
      {
        bug: 'NullPointerException when input is null',
        steps: ['Call processData(null)', 'Observe crash'],
        expected: 'Should throw ValidationError with helpful message',
        actual: 'Crashes with NPE on line 42',
        severity: 'critical',
        test_case: 'test_null_input_throws_validation_error()',
      },
      {
        bug: 'Race condition in concurrent access',
        steps: ['Start 10 threads', 'Each thread calls update()', 'Check final state'],
        expected: 'Final count should be 10',
        actual: 'Count varies between 7-10 due to race condition',
        severity: 'major',
        test_case: 'test_concurrent_update_is_thread_safe()',
      },
    ];

    // Publish VALIDATION_RESULT with detailed issues (as validator hook would)
    messageBus.publish({
      cluster_id: clusterId,
      topic: 'VALIDATION_RESULT',
      sender: 'tester',
      content: {
        text: '2 bugs found - rejecting implementation',
        data: {
          approved: false,
          issues: detailedIssues,
        },
      },
    });

    // Create worker agent with contextStrategy that includes VALIDATION_RESULT
    const workerConfig = {
      id: 'worker',
      role: 'implementation',
      modelLevel: 'level2',
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

    const worker = new AgentWrapper(workerConfig, messageBus, mockCluster, {
      testMode: true,
      mockSpawnFn: () => {},
    });

    // Build context as if triggered by a message
    const context = worker._buildContext({
      cluster_id: clusterId,
      topic: 'VALIDATION_RESULT',
      sender: 'tester',
      content: { text: 'Rejection trigger' },
    });

    // CRITICAL ASSERTIONS: Verify ALL bug details are in context
    console.log('\n=== CONTEXT CONTENT ===');
    console.log(context);
    console.log('=== END CONTEXT ===\n');

    // Check bug descriptions
    assert(
      context.includes('NullPointerException when input is null'),
      'Context must include first bug description'
    );
    assert(
      context.includes('Race condition in concurrent access'),
      'Context must include second bug description'
    );

    // Check reproduction steps
    assert(context.includes('Call processData(null)'), 'Context must include reproduction steps');
    assert(
      context.includes('Start 10 threads'),
      'Context must include second bug reproduction steps'
    );

    // Check expected/actual behavior
    assert(
      context.includes('Should throw ValidationError'),
      'Context must include expected behavior'
    );
    assert(context.includes('Crashes with NPE on line 42'), 'Context must include actual behavior');
    assert(
      context.includes('race condition'),
      'Context must include actual behavior for second bug'
    );

    // Check severity
    assert(context.includes('critical'), 'Context must include severity');
    assert(context.includes('major'), 'Context must include second severity');

    // Check test cases
    assert(
      context.includes('test_null_input_throws_validation_error()'),
      'Context must include required test case'
    );
    assert(
      context.includes('test_concurrent_update_is_thread_safe()'),
      'Context must include second required test case'
    );

    console.log('✅ ALL rejection details are FULLY injected into worker context!');
  });
}

function registerMultipleRejectionsContextTest() {
  it('should include multiple validator rejections in worker context', () => {
    const clusterId = 'test-cluster-456';
    const clusterCreatedAt = Date.now() - 60000;

    // Multiple validators rejecting
    messageBus.publish({
      cluster_id: clusterId,
      topic: 'VALIDATION_RESULT',
      sender: 'requirements-validator',
      content: {
        text: 'Missing authentication for admin endpoint',
        data: {
          approved: false,
          errors: ['POST /admin/delete-user has no auth check', 'JWT validation missing'],
        },
      },
    });

    messageBus.publish({
      cluster_id: clusterId,
      topic: 'VALIDATION_RESULT',
      sender: 'code-reviewer',
      content: {
        text: 'Security vulnerability found',
        data: {
          approved: false,
          issues: [
            {
              bug: 'SQL injection in search query',
              steps: ["Enter: '; DROP TABLE users; --"],
              expected: 'Input should be escaped',
              actual: 'Query executes unescaped',
              severity: 'critical',
              test_case: 'test_search_escapes_sql()',
            },
          ],
        },
      },
    });

    const workerConfig = {
      id: 'worker',
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

    const worker = new AgentWrapper(workerConfig, messageBus, mockCluster, {
      testMode: true,
      mockSpawnFn: () => {},
    });

    const context = worker._buildContext({
      cluster_id: clusterId,
      topic: 'VALIDATION_RESULT',
      sender: 'code-reviewer',
      content: { text: 'trigger' },
    });

    // Verify BOTH validators' feedback is included
    assert(
      context.includes('requirements-validator'),
      'Context must include first validator sender'
    );
    assert(context.includes('code-reviewer'), 'Context must include second validator sender');

    assert(
      context.includes('POST /admin/delete-user has no auth check'),
      'Context must include first validator errors'
    );
    assert(
      context.includes('SQL injection in search query'),
      'Context must include second validator issues'
    );
    assert(
      context.includes('DROP TABLE users'),
      'Context must include reproduction steps from second validator'
    );

    console.log('✅ MULTIPLE validator rejections are ALL injected into worker context!');
  });
}
