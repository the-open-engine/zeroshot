/**
 * Substitute Template Tests
 *
 * Tests for template variable substitution in hooks, including:
 * - Known variables ({{cluster.id}}, {{result.*}}, etc.)
 * - Content containing arbitrary {{...}} patterns (React, Mustache, etc.)
 */

const assert = require('assert');
const { substituteTemplate } = require('../src/agent/agent-hook-executor');

// Mock agent for testing
const createMockAgent = (overrides = {}) => ({
  id: 'test-agent',
  role: 'implementation',
  iteration: 1,
  cluster_id: 'test-cluster',
  _parseResultOutput: (output) => {
    // Extract JSON block from output
    const match = output.match(/```json\s*([\s\S]*?)\s*```/);
    if (match) {
      return JSON.parse(match[1]);
    }
    return JSON.parse(output);
  },
  ...overrides,
});

const mockCluster = { id: 'test-cluster-123', createdAt: 1234567890 };

describe('substituteTemplate', () => {
  defineKnownVariableTests();
  defineArbitraryPatternTests();
  defineEdgeCaseTests();
});

function defineKnownVariableTests() {
  describe('known template variables', () => {
    it('should substitute {{cluster.id}}', async () => {
      const config = {
        topic: 'TEST',
        content: { text: 'Cluster: {{cluster.id}}' },
      };
      const context = {};
      const agent = createMockAgent();

      const result = await substituteTemplate({
        config,
        context,
        agent,
        cluster: mockCluster,
      });

      assert.strictEqual(result.content.text, 'Cluster: test-cluster-123');
    });

    it('should substitute {{iteration}}', async () => {
      const config = {
        topic: 'TEST',
        content: { text: 'Iteration: {{iteration}}' },
      };
      const context = {};
      const agent = createMockAgent({ iteration: 5 });

      const result = await substituteTemplate({
        config,
        context,
        agent,
        cluster: mockCluster,
      });

      assert.strictEqual(result.content.text, 'Iteration: 5');
    });

    it('should substitute {{result.*}} from parsed output', async () => {
      const config = {
        topic: 'PLAN_READY',
        content: {
          text: '{{result.plan}}',
          data: { summary: '{{result.summary}}' },
        },
      };
      const context = {
        result: {
          output: '```json\n{"plan": "Step 1", "summary": "Test summary"}\n```',
        },
      };
      const agent = createMockAgent();

      const result = await substituteTemplate({
        config,
        context,
        agent,
        cluster: mockCluster,
      });

      assert.strictEqual(result.content.text, 'Step 1');
      assert.strictEqual(result.content.data.summary, 'Test summary');
    });

    it('should fail on unsubstituted known variables', async () => {
      const config = {
        topic: 'TEST',
        content: { text: '{{result.missing}}' },
      };
      const context = {
        result: {
          output: '```json\n{"other": "value"}\n```',
        },
      };
      const agent = createMockAgent();

      // Should NOT throw - missing result fields now default to null with warning
      const result = await substituteTemplate({
        config,
        context,
        agent,
        cluster: mockCluster,
      });

      // Missing fields become null
      assert.strictEqual(result.content.text, null);
    });
  });
}

function defineArbitraryPatternTests() {
  describe('arbitrary {{...}} patterns in content', () => {
    it('should allow React dangerouslySetInnerHTML patterns in content', async () => {
      const config = {
        topic: 'PLAN_READY',
        content: {
          text: '{{result.plan}}',
        },
      };
      const context = {
        result: {
          // This is the REAL bug scenario: agent output contains React code
          output: `\`\`\`json
{
  "plan": "Use dangerouslySetInnerHTML={{__html: userInput}} to render HTML"
}
\`\`\``,
        },
      };
      const agent = createMockAgent();

      // Should NOT throw on {{__html: userInput}} - it's just content
      const result = await substituteTemplate({
        config,
        context,
        agent,
        cluster: mockCluster,
      });

      assert.ok(result.content.text.includes('{{__html: userInput}}'));
    });

    it('should allow Mustache template patterns in content', async () => {
      const config = {
        topic: 'PLAN_READY',
        content: {
          text: '{{result.plan}}',
        },
      };
      const context = {
        result: {
          output: `\`\`\`json
{
  "plan": "Edit template to use {{#items}}{{name}}{{/items}}"
}
\`\`\``,
        },
      };
      const agent = createMockAgent();

      // Should NOT throw on Mustache patterns
      const result = await substituteTemplate({
        config,
        context,
        agent,
        cluster: mockCluster,
      });

      assert.ok(result.content.text.includes('{{#items}}'));
    });

    it('should allow Handlebars helper patterns in content', async () => {
      const config = {
        topic: 'PLAN_READY',
        content: {
          text: '{{result.plan}}',
        },
      };
      const context = {
        result: {
          output: `\`\`\`json
{
  "plan": "Use {{formatDate date 'YYYY-MM-DD'}} helper"
}
\`\`\``,
        },
      };
      const agent = createMockAgent();

      // Should NOT throw on Handlebars patterns
      const result = await substituteTemplate({
        config,
        context,
        agent,
        cluster: mockCluster,
      });

      assert.ok(result.content.text.includes('{{formatDate'));
    });

    it('should still catch unsubstituted KNOWN variables even with arbitrary patterns', async () => {
      const config = {
        topic: 'TEST',
        // Mix of known variable (should fail) and arbitrary pattern (should be ignored)
        content: { text: '{{result.plan}} and {{arbitrary.thing}}' },
      };
      const context = {}; // No result - should fail on {{result.plan}}

      const agent = createMockAgent();

      // Should throw because {{result.plan}} is a KNOWN pattern but no result provided
      await assert.rejects(
        () =>
          substituteTemplate({
            config,
            context,
            agent,
            cluster: mockCluster,
          }),
        /result\.\* variables but no result/
      );
    });
  });
}

function defineEdgeCaseTests() {
  describe('edge cases', () => {
    it('should handle empty config', async () => {
      await assert.rejects(
        () =>
          substituteTemplate({
            config: null,
            context: {},
            agent: createMockAgent(),
            cluster: mockCluster,
          }),
        /config is required/
      );
    });

    it('should handle result with boolean values', async () => {
      const config = {
        topic: 'VALIDATION_RESULT',
        content: {
          data: { approved: '{{result.approved}}' },
        },
      };
      const context = {
        result: {
          output: '```json\n{"approved": true}\n```',
        },
      };
      const agent = createMockAgent();

      const result = await substituteTemplate({
        config,
        context,
        agent,
        cluster: mockCluster,
      });

      // Boolean should be unquoted in JSON
      assert.strictEqual(result.content.data.approved, true);
    });

    it('should handle nested braces in content correctly', async () => {
      const config = {
        topic: 'PLAN_READY',
        content: {
          text: '{{result.code}}',
        },
      };
      const context = {
        result: {
          output: `\`\`\`json
{
  "code": "const obj = { key: '{{value}}' };"
}
\`\`\``,
        },
      };
      const agent = createMockAgent();

      // Should handle nested braces without breaking
      const result = await substituteTemplate({
        config,
        context,
        agent,
        cluster: mockCluster,
      });

      assert.ok(result.content.text.includes('{{value}}'));
    });
  });
}
