/**
 * Test: maxModel Ceiling Enforcement
 *
 * Tests the cost ceiling mechanism that prevents agents from using
 * models more expensive than the configured maxModel setting.
 *
 * Key behaviors tested:
 * - Static model exceeds ceiling → ERROR at agent spawn
 * - Dynamic modelRules exceed ceiling → ERROR at config validation
 * - Unspecified model defaults to provider default level (bounded by maxModel)
 * - Models within ceiling are allowed
 */

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const os = require('os');

// Test storage directory (isolated)
const TEST_STORAGE_DIR = path.join(os.tmpdir(), 'zeroshot-maxmodel-test-' + Date.now());
const TEST_SETTINGS_FILE = path.join(TEST_STORAGE_DIR, 'settings.json');

let AgentWrapper;
let validateAgentConfig;
let settingsModule;
let mockMessageBus;
let mockCluster;

function saveTestSettings(settings) {
  fs.writeFileSync(TEST_SETTINGS_FILE, JSON.stringify(settings, null, 2), 'utf8');
}

function registerMaxModelHooks() {
  before(function () {
    if (!fs.existsSync(TEST_STORAGE_DIR)) {
      fs.mkdirSync(TEST_STORAGE_DIR, { recursive: true });
    }

    process.env.ZEROSHOT_SETTINGS_FILE = TEST_SETTINGS_FILE;

    delete require.cache[require.resolve('../lib/settings')];
    delete require.cache[require.resolve('../src/agent/agent-config')];
    delete require.cache[require.resolve('../src/agent-wrapper')];

    settingsModule = require('../lib/settings');
    validateAgentConfig = require('../src/agent/agent-config').validateAgentConfig;
    AgentWrapper = require('../src/agent-wrapper');

    mockMessageBus = {
      publish: () => {},
      subscribe: () => () => {},
      subscribeTopic: () => () => {},
      query: () => [],
      findLast: () => null,
      count: () => 0,
    };
    mockCluster = { id: 'test-cluster', agents: [] };
  });

  after(function () {
    try {
      fs.rmSync(TEST_STORAGE_DIR, { recursive: true, force: true });
    } catch (e) {
      console.error('Cleanup failed:', e.message);
    }
    delete process.env.ZEROSHOT_SETTINGS_FILE;
  });

  beforeEach(function () {
    if (fs.existsSync(TEST_SETTINGS_FILE)) {
      fs.unlinkSync(TEST_SETTINGS_FILE);
    }
  });
}

function registerStaticModelCeilingTests() {
  describe('Static model ceiling enforcement', function () {
    it('should ERROR when agent requests opus but maxModel is sonnet', function () {
      // Set maxModel to sonnet
      saveTestSettings({ maxModel: 'sonnet' });

      // Agent config requests opus
      const agentConfig = { id: 'worker', model: 'opus', timeout: 0 };

      // Should throw at config validation time
      assert.throws(
        () => validateAgentConfig(agentConfig, { testMode: true, mockSpawnFn: () => {} }),
        /Agent "worker":.*opus.*maxModel is "sonnet"/
      );
    });

    it('should ERROR when agent requests opus but maxModel is haiku', function () {
      saveTestSettings({ maxModel: 'haiku' });

      const agentConfig = { id: 'expensive-agent', model: 'opus', timeout: 0 };

      assert.throws(
        () => validateAgentConfig(agentConfig, { testMode: true, mockSpawnFn: () => {} }),
        /Agent "expensive-agent":.*opus.*maxModel is "haiku"/
      );
    });

    it('should ERROR when agent requests sonnet but maxModel is haiku', function () {
      saveTestSettings({ maxModel: 'haiku' });

      const agentConfig = { id: 'mid-tier-agent', model: 'sonnet', timeout: 0 };

      assert.throws(
        () => validateAgentConfig(agentConfig, { testMode: true, mockSpawnFn: () => {} }),
        /Agent "mid-tier-agent":.*sonnet.*maxModel is "haiku"/
      );
    });

    it('should allow agent to request LOWER model than maxModel', function () {
      saveTestSettings({ maxModel: 'opus' });

      // Agent requests haiku (lower than opus ceiling)
      const agentConfig = { id: 'frugal-agent', model: 'haiku', timeout: 0 };

      // Should NOT throw - haiku < opus is fine
      assert.doesNotThrow(() => {
        validateAgentConfig(agentConfig, { testMode: true, mockSpawnFn: () => {} });
      });
    });

    it('should allow agent to request same model as maxModel', function () {
      saveTestSettings({ maxModel: 'sonnet' });

      const agentConfig = { id: 'standard-agent', model: 'sonnet', timeout: 0 };

      assert.doesNotThrow(() => {
        validateAgentConfig(agentConfig, { testMode: true, mockSpawnFn: () => {} });
      });
    });
  });
}

function registerDynamicModelRulesTests() {
  describe('Dynamic modelRules ceiling enforcement', function () {
    it('should ERROR when any modelRule exceeds maxModel', function () {
      saveTestSettings({ maxModel: 'sonnet' });

      // Agent has rules that escalate to opus on iteration 2+
      const agentConfig = {
        id: 'escalating-worker',
        timeout: 0,
        modelRules: [
          { iterations: '1', model: 'sonnet' },
          { iterations: '2+', model: 'opus' }, // VIOLATION
        ],
      };

      // Should catch this NOW, not at iteration 2
      assert.throws(
        () => validateAgentConfig(agentConfig, { testMode: true, mockSpawnFn: () => {} }),
        /Agent "escalating-worker":.*modelRule "2\+".*opus.*maxModel is "sonnet"/
      );
    });

    it('should ERROR when first modelRule exceeds maxModel', function () {
      saveTestSettings({ maxModel: 'haiku' });

      const agentConfig = {
        id: 'fast-start-worker',
        timeout: 0,
        modelRules: [
          { iterations: '1', model: 'sonnet' }, // VIOLATION on first iteration
          { iterations: '2+', model: 'haiku' },
        ],
      };

      assert.throws(
        () => validateAgentConfig(agentConfig, { testMode: true, mockSpawnFn: () => {} }),
        /Agent "fast-start-worker":.*modelRule "1".*sonnet.*maxModel is "haiku"/
      );
    });

    it('should allow modelRules all within ceiling', function () {
      saveTestSettings({ maxModel: 'opus' });

      const agentConfig = {
        id: 'flexible-worker',
        timeout: 0,
        modelRules: [
          { iterations: '1', model: 'haiku' },
          { iterations: '2-5', model: 'sonnet' },
          { iterations: '6+', model: 'opus' },
        ],
      };

      assert.doesNotThrow(() => {
        validateAgentConfig(agentConfig, { testMode: true, mockSpawnFn: () => {} });
      });
    });

    it('should validate ALL rules at config time, not just first', function () {
      saveTestSettings({ maxModel: 'sonnet' });

      // Late escalation rule should still be caught at config time
      const agentConfig = {
        id: 'late-escalator',
        timeout: 0,
        modelRules: [
          { iterations: '1', model: 'haiku' },
          { iterations: '2-10', model: 'sonnet' },
          { iterations: '11+', model: 'opus' }, // VIOLATION at iteration 11
        ],
      };

      // Should catch this NOW, not at iteration 11
      assert.throws(
        () => validateAgentConfig(agentConfig, { testMode: true, mockSpawnFn: () => {} }),
        /modelRule "11\+".*opus.*maxModel is "sonnet"/
      );
    });
  });
}

function registerDefaultModelTests() {
  describe('Default model when unspecified', function () {
    it('should use maxModel ceiling when it constrains default', function () {
      saveTestSettings({ maxModel: 'haiku' });

      const agentConfig = { id: 'default-model-agent', timeout: 0 };

      // Config should pass validation
      const _normalized = validateAgentConfig(agentConfig, {
        testMode: true,
        mockSpawnFn: () => {},
      });

      // Create agent and check model selection
      const agent = new AgentWrapper(agentConfig, mockMessageBus, mockCluster, {
        testMode: true,
        mockSpawnFn: () => {},
      });

      assert.strictEqual(agent._selectModel(), 'haiku');
    });

    it('should use provider default level when unspecified in settings', function () {
      // Don't save any settings - use defaults
      if (fs.existsSync(TEST_SETTINGS_FILE)) {
        fs.unlinkSync(TEST_SETTINGS_FILE);
      }

      const agentConfig = { id: 'no-settings-agent', timeout: 0 };

      const agent = new AgentWrapper(agentConfig, mockMessageBus, mockCluster, {
        testMode: true,
        mockSpawnFn: () => {},
      });

      // Default provider level is level2 (sonnet for claude)
      assert.strictEqual(agent._selectModel(), 'sonnet');
    });

    it('should use provider default level even when maxModel allows higher', function () {
      saveTestSettings({ maxModel: 'opus' });

      const agentConfig = { id: 'premium-default-agent', timeout: 0 };

      const agent = new AgentWrapper(agentConfig, mockMessageBus, mockCluster, {
        testMode: true,
        mockSpawnFn: () => {},
      });

      assert.strictEqual(agent._selectModel(), 'sonnet');
    });
  });
}

function registerSettingsValidationTests() {
  describe('Settings validation', function () {
    it('should reject invalid maxModel values', function () {
      const error = settingsModule.validateSetting('maxModel', 'gpt4');
      assert.ok(error !== null);
      assert.ok(error.includes('Invalid model'));
      assert.ok(error.includes('opus, sonnet, haiku'));
    });

    it('should accept valid maxModel values', function () {
      assert.strictEqual(settingsModule.validateSetting('maxModel', 'opus'), null);
      assert.strictEqual(settingsModule.validateSetting('maxModel', 'sonnet'), null);
      assert.strictEqual(settingsModule.validateSetting('maxModel', 'haiku'), null);
    });
  });
}

function registerModelHierarchyValidationTests() {
  describe('Model hierarchy validation function', function () {
    it('should return requested model when within ceiling', function () {
      const result = settingsModule.validateModelAgainstMax('haiku', 'opus');
      assert.strictEqual(result, 'haiku');
    });

    it('should return maxModel when requested model is null/undefined', function () {
      assert.strictEqual(settingsModule.validateModelAgainstMax(null, 'sonnet'), 'sonnet');
      assert.strictEqual(settingsModule.validateModelAgainstMax(undefined, 'opus'), 'opus');
    });

    it('should throw for invalid requested model', function () {
      assert.throws(
        () => settingsModule.validateModelAgainstMax('gpt4', 'sonnet'),
        /Invalid model "gpt4"/
      );
    });

    it('should throw for invalid maxModel', function () {
      assert.throws(
        () => settingsModule.validateModelAgainstMax('sonnet', 'claude-3'),
        /Invalid maxModel "claude-3"/
      );
    });

    it('should throw when requested exceeds ceiling', function () {
      assert.throws(
        () => settingsModule.validateModelAgainstMax('opus', 'sonnet'),
        /Agent requests "opus" but maxModel is "sonnet"/
      );
    });
  });
}

function registerIntegrationCeilingTests() {
  describe('Integration: Full agent lifecycle with ceiling', function () {
    it('should enforce ceiling throughout agent execution', function () {
      saveTestSettings({ maxModel: 'sonnet' });

      // Create agent with modelRules that stay within ceiling
      const agentConfig = {
        id: 'lifecycle-test',
        timeout: 0,
        modelRules: [
          { iterations: '1', model: 'haiku' },
          { iterations: '2+', model: 'sonnet' },
        ],
      };

      const agent = new AgentWrapper(agentConfig, mockMessageBus, mockCluster, {
        testMode: true,
        mockSpawnFn: () => {},
      });

      // Verify model selection at different iterations
      agent.iteration = 1;
      assert.strictEqual(agent._selectModel(), 'haiku');

      agent.iteration = 2;
      assert.strictEqual(agent._selectModel(), 'sonnet');

      agent.iteration = 10;
      assert.strictEqual(agent._selectModel(), 'sonnet');
    });

    it('should cap model at ceiling even when rules say otherwise', function () {
      // This tests the runtime _selectModel enforcement in agent-wrapper.js
      // First, we create an agent with valid config
      saveTestSettings({ maxModel: 'opus' });

      const agentConfig = {
        id: 'runtime-ceiling-test',
        timeout: 0,
        model: 'sonnet',
      };

      const agent = new AgentWrapper(agentConfig, mockMessageBus, mockCluster, {
        testMode: true,
        mockSpawnFn: () => {},
      });

      // Verify static model is used correctly
      assert.strictEqual(agent._selectModel(), 'sonnet');

      // Now change settings to lower ceiling
      // Note: This simulates a race condition where settings change after agent starts
      // The agent should respect the ceiling at runtime
      saveTestSettings({ maxModel: 'haiku' });

      // Clear the settings cache by reloading
      delete require.cache[require.resolve('../lib/settings')];

      // The agent's _selectModel calls loadSettings each time, so it should now fail
      assert.throws(() => agent._selectModel(), /Agent requests "sonnet" but maxModel is "haiku"/);
    });
  });
}

function registerModelHierarchyConstantTests() {
  describe('MODEL_HIERARCHY constant', function () {
    it('should have correct hierarchy values', function () {
      const { MODEL_HIERARCHY } = settingsModule;

      assert.strictEqual(MODEL_HIERARCHY.opus, 3);
      assert.strictEqual(MODEL_HIERARCHY.sonnet, 2);
      assert.strictEqual(MODEL_HIERARCHY.haiku, 1);
    });

    it('should have all valid models in hierarchy', function () {
      const { MODEL_HIERARCHY, VALID_MODELS } = settingsModule;

      for (const model of VALID_MODELS) {
        assert.ok(model in MODEL_HIERARCHY, `Model ${model} should be in hierarchy`);
      }
    });
  });
}

describe('maxModel Ceiling Enforcement', function () {
  this.timeout(10000);

  registerMaxModelHooks();
  registerStaticModelCeilingTests();
  registerDynamicModelRulesTests();
  registerDefaultModelTests();
  registerSettingsValidationTests();
  registerModelHierarchyValidationTests();
  registerIntegrationCeilingTests();
  registerModelHierarchyConstantTests();
});
