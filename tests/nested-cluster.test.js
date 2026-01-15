/**
 * Nested Cluster Tests
 *
 * Tests for sub-cluster functionality:
 * 1. Config validation for subclusters
 * 2. Max nesting depth enforced
 * 3. SubClusterWrapper integration
 * 4. Message bridging setup
 * 5. Example config validation
 */

const assert = require('assert');
const Orchestrator = require('../src/orchestrator');
const SubClusterWrapper = require('../src/sub-cluster-wrapper');
const MessageBusBridge = require('../src/message-bus-bridge');
const MessageBus = require('../src/message-bus');
const Ledger = require('../src/ledger');
const path = require('path');
const os = require('os');
const fs = require('fs');

// Test storage directory (cleaned up after each test)
const TEST_STORAGE = path.join(os.tmpdir(), 'zeroshot-nested-tests');

let orchestrator;

describe('Nested Cluster', function () {
  this.timeout(15000);

  beforeEach(() => {
    // Clean test directory
    if (fs.existsSync(TEST_STORAGE)) {
      fs.rmSync(TEST_STORAGE, { recursive: true, force: true });
    }
    fs.mkdirSync(TEST_STORAGE, { recursive: true });

    orchestrator = new Orchestrator({
      skipLoad: true,
      quiet: true,
      storageDir: TEST_STORAGE,
    });
  });

  afterEach(async () => {
    // Clean up clusters
    try {
      await orchestrator.killAll();
    } catch {
      /* ignore */
    }

    try {
      orchestrator.close();
    } catch {
      /* ignore */
    }

    // Clean test directory
    if (fs.existsSync(TEST_STORAGE)) {
      fs.rmSync(TEST_STORAGE, { recursive: true, force: true });
    }
  });

  defineConfigValidationTests();
  defineMaxNestingDepthTests();
  defineSubClusterWrapperTests();
  defineMessageBusBridgeTests();
});

function defineConfigValidationTests() {
  describe('Config Validation', function () {
    it('should validate valid subcluster config', function () {
      const config = {
        agents: [
          {
            id: 'parent',
            role: 'planning',
            modelLevel: 'level1',
            triggers: [{ topic: 'ISSUE_OPENED' }],
            hooks: {
              onComplete: {
                action: 'publish_message',
                config: { topic: 'PLAN_READY', content: { text: 'Done' } },
              },
            },
          },
          {
            id: 'child-cluster',
            type: 'subcluster',
            role: 'orchestrator',
            config: {
              agents: [
                {
                  id: 'worker',
                  role: 'implementation',
                  modelLevel: 'level1',
                  triggers: [{ topic: 'ISSUE_OPENED' }],
                  hooks: {
                    onComplete: {
                      action: 'publish_message',
                      config: {
                        topic: 'WORK_DONE',
                        content: { text: 'Work complete' },
                      },
                    },
                  },
                },
                {
                  id: 'child-completion',
                  role: 'orchestrator',
                  triggers: [
                    {
                      topic: 'WORK_DONE',
                      action: 'stop_cluster',
                    },
                  ],
                },
              ],
            },
            triggers: [{ topic: 'PLAN_READY' }],
            hooks: {
              onComplete: {
                action: 'publish_message',
                config: { topic: 'DONE', content: { text: 'Complete' } },
              },
            },
          },
          {
            id: 'parent-completion',
            role: 'orchestrator',
            triggers: [
              {
                topic: 'DONE',
                action: 'stop_cluster',
              },
            ],
          },
        ],
      };

      const result = orchestrator.validateConfig(config);
      assert.strictEqual(
        result.valid,
        true,
        `Config should be valid. Errors: ${JSON.stringify(result.errors)}`
      );
    });

    it('should reject subcluster with missing config', function () {
      const config = {
        agents: [
          {
            id: 'bad-subcluster',
            type: 'subcluster',
            role: 'orchestrator',
            // Missing config field
            triggers: [{ topic: 'START' }],
          },
        ],
      };

      const result = orchestrator.validateConfig(config);
      assert.strictEqual(result.valid, false);
      assert(result.errors.some((e) => e.toLowerCase().includes('config')));
    });

    it('should reject subcluster with empty agents array', function () {
      const config = {
        agents: [
          {
            id: 'empty-subcluster',
            type: 'subcluster',
            role: 'orchestrator',
            config: {
              agents: [], // Empty
            },
            triggers: [{ topic: 'START' }],
          },
        ],
      };

      const result = orchestrator.validateConfig(config);
      assert.strictEqual(result.valid, false);
      assert(result.errors.some((e) => e.toLowerCase().includes('empty')));
    });

    it('should reject subcluster with missing triggers', function () {
      const config = {
        agents: [
          {
            id: 'no-trigger-subcluster',
            type: 'subcluster',
            role: 'orchestrator',
            config: {
              agents: [
                {
                  id: 'worker',
                  role: 'implementation',
                  modelLevel: 'level1',
                  triggers: [{ topic: 'START' }],
                },
              ],
            },
            // Missing triggers
          },
        ],
      };

      const result = orchestrator.validateConfig(config);
      assert.strictEqual(result.valid, false);
      assert(result.errors.some((e) => e.toLowerCase().includes('trigger')));
    });
  });
}

function defineMaxNestingDepthTests() {
  describe('Max Nesting Depth', function () {
    it('should enforce max nesting depth of 5', function () {
      // Create deeply nested config (7 levels deep - should fail at 5)
      const createNestedConfig = (depth) => {
        if (depth > 7) {
          return {
            agents: [
              {
                id: `leaf-${depth}`,
                role: 'implementation',
                modelLevel: 'level1',
                triggers: [{ topic: 'ISSUE_OPENED' }],
              },
            ],
          };
        }

        return {
          agents: [
            {
              id: `subcluster-${depth}`,
              type: 'subcluster',
              role: 'orchestrator',
              config: createNestedConfig(depth + 1),
              triggers: [{ topic: 'ISSUE_OPENED' }],
            },
          ],
        };
      };

      const config = createNestedConfig(1);

      // Validate config - should fail with max depth error
      const validation = orchestrator.validateConfig(config);
      assert.strictEqual(validation.valid, false);
      assert(
        validation.errors.some(
          (e) =>
            e.toLowerCase().includes('max') &&
            (e.toLowerCase().includes('depth') || e.toLowerCase().includes('nesting'))
        )
      );
    });
  });
}

function defineSubClusterWrapperTests() {
  describe('SubClusterWrapper', function () {
    it('should create SubClusterWrapper instance', function () {
      const dbPath = path.join(TEST_STORAGE, 'test.db');
      const ledger = new Ledger(dbPath);
      const messageBus = new MessageBus(ledger);

      const config = {
        id: 'test-subcluster',
        type: 'subcluster',
        role: 'orchestrator',
        config: {
          agents: [
            {
              id: 'worker',
              role: 'implementation',
              modelLevel: 'level1',
              triggers: [{ topic: 'START' }],
            },
          ],
        },
        triggers: [{ topic: 'BEGIN' }],
      };

      const wrapper = new SubClusterWrapper(
        config,
        messageBus,
        { id: 'parent-cluster' },
        { quiet: true }
      );

      assert.strictEqual(wrapper.id, 'test-subcluster');
      assert.strictEqual(wrapper.role, 'orchestrator');
      assert.strictEqual(wrapper.state, 'idle');
    });
  });
}

function defineMessageBusBridgeTests() {
  describe('MessageBusBridge', function () {
    it('should create MessageBusBridge instance', function () {
      const dbPath1 = path.join(TEST_STORAGE, 'parent.db');
      const dbPath2 = path.join(TEST_STORAGE, 'child.db');
      const parentLedger = new Ledger(dbPath1);
      const childLedger = new Ledger(dbPath2);
      const parentBus = new MessageBus(parentLedger);
      const childBus = new MessageBus(childLedger);

      const bridge = new MessageBusBridge(parentBus, childBus, {
        parentClusterId: 'parent-123',
        childClusterId: 'child-456',
        parentTopics: ['ISSUE_OPENED', 'PLAN_READY'],
      });

      assert.strictEqual(bridge.isActive(), true);

      bridge.close();
      assert.strictEqual(bridge.isActive(), false);
    });
  });
}
