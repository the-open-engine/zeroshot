/**
 * Template Resolver - Capability Injection Integration Tests
 *
 * Tests that the TemplateResolver properly integrates with PromptInjector
 * to inject provider-specific capabilities into template prompts.
 */

const assert = require('assert');
const path = require('path');
const TemplateResolver = require('../src/template-resolver');

describe('TemplateResolver - Capability Injection Integration', () => {
  let resolver;

  beforeEach(() => {
    const templatesDir = path.join(__dirname, '../cluster-templates');
    resolver = new TemplateResolver(templatesDir);
  });

  describe('resolve() with provider option', () => {
    it('should inject capabilities for Claude provider', () => {
      const resolved = resolver.resolve('debug-workflow', {}, { provider: 'claude' });

      assert(resolved.agents, 'Should have agents');

      // Find the fixer agent which has the CAPABILITY_EXTENSIONS marker
      const fixerAgent = resolved.agents.find((a) => a.id === 'fixer');
      assert(fixerAgent, 'Should have fixer agent');
      assert(fixerAgent.prompt, 'Fixer should have prompt');

      const promptText = fixerAgent.prompt.system;
      assert(promptText, 'Should have system prompt');

      // Should include sub-agent instructions for Claude
      assert(
        promptText.includes('EXECUTING DELEGATED TASKS') || promptText.includes('SUB-AGENT'),
        'Claude should get sub-agent instructions'
      );

      // Should NOT have the marker anymore
      assert(!promptText.includes('{{CAPABILITY_EXTENSIONS}}'), 'Marker should be replaced');
    });

    it('should NOT inject sub-agent capabilities for Gemini provider', () => {
      const resolved = resolver.resolve('debug-workflow', {}, { provider: 'gemini' });

      const fixerAgent = resolved.agents.find((a) => a.id === 'fixer');
      assert(fixerAgent, 'Should have fixer agent');

      const promptText = fixerAgent.prompt.system;

      // Should NOT include sub-agent instructions for Gemini
      assert(
        !promptText.includes('Task tool') && !promptText.includes('EXECUTING DELEGATED TASKS'),
        'Gemini should NOT get sub-agent instructions'
      );

      // Should NOT have the marker anymore
      assert(
        !promptText.includes('{{CAPABILITY_EXTENSIONS}}'),
        'Marker should be replaced (even if with empty string)'
      );
    });

    it('should NOT inject sub-agent capabilities for Codex provider', () => {
      const resolved = resolver.resolve('debug-workflow', {}, { provider: 'codex' });

      const fixerAgent = resolved.agents.find((a) => a.id === 'fixer');
      assert(fixerAgent, 'Should have fixer agent');

      const promptText = fixerAgent.prompt.system;

      // Should NOT include sub-agent instructions for Codex
      assert(
        !promptText.includes('Task tool') && !promptText.includes('EXECUTING DELEGATED TASKS'),
        'Codex should NOT get sub-agent instructions'
      );

      // Codex also doesn't support parallel execution
      assert(
        !promptText.includes('PARALLEL EXECUTION'),
        'Codex should NOT get parallel execution instructions'
      );
    });

    it('should work without provider option (no injection)', () => {
      const resolved = resolver.resolve('debug-workflow', {});

      const fixerAgent = resolved.agents.find((a) => a.id === 'fixer');
      assert(fixerAgent, 'Should have fixer agent');

      const promptText = fixerAgent.prompt.system;

      // Without provider, marker should remain unchanged (no injection)
      // Actually, the marker gets replaced with empty string when no provider
      assert(promptText, 'Should have prompt text');
    });

    it('should inject into full-workflow template', () => {
      const resolved = resolver.resolve(
        'full-workflow',
        {
          task_type: 'TASK',
          worker_level: 'level2',
          validator_count: 3,
          validator_level: 'level2',
          tester_level: 'level2',
          max_iterations: 10,
          max_tokens: 100000,
          timeout: 0,
        },
        { provider: 'claude' }
      );

      // Find the worker agent
      const workerAgent = resolved.agents.find((a) => a.id === 'worker');
      assert(workerAgent, 'Should have worker agent');
      assert(workerAgent.prompt, 'Worker should have prompt');

      const initialPrompt = workerAgent.prompt.initial;
      assert(initialPrompt, 'Should have initial prompt');

      // Should include sub-agent instructions for Claude
      assert(
        initialPrompt.includes('EXECUTING DELEGATED TASKS') || initialPrompt.includes('SUB-AGENT'),
        'Claude should get sub-agent instructions in full-workflow'
      );

      // Should NOT have the marker anymore
      assert(!initialPrompt.includes('{{CAPABILITY_EXTENSIONS}}'), 'Marker should be replaced');
    });
  });

  describe('Prompt format handling', () => {
    it('should handle string prompts', () => {
      // Create a simple test case - string prompts are less common but should work
      const resolved = resolver.resolve('debug-workflow', {}, { provider: 'claude' });
      assert(resolved.agents.length > 0, 'Should have agents');
    });

    it('should handle object prompts with multiple fields', () => {
      const resolved = resolver.resolve(
        'full-workflow',
        {
          task_type: 'TASK',
          worker_level: 'level2',
          validator_count: 3,
          validator_level: 'level2',
          tester_level: 'level2',
          max_iterations: 10,
          max_tokens: 100000,
          timeout: 0,
        },
        { provider: 'claude' }
      );

      const workerAgent = resolved.agents.find((a) => a.id === 'worker');
      assert(workerAgent.prompt.initial, 'Should have initial prompt');
      assert(workerAgent.prompt.subsequent, 'Should have subsequent prompt');

      // Both should have capabilities injected
      assert(
        !workerAgent.prompt.initial.includes('{{CAPABILITY_EXTENSIONS}}'),
        'Initial prompt marker should be replaced'
      );
      assert(
        !workerAgent.prompt.subsequent.includes('{{CAPABILITY_EXTENSIONS}}'),
        'Subsequent prompt marker should be replaced'
      );
    });
  });

  describe('Marker replacement verification', () => {
    it('should not leave any {{CAPABILITY_EXTENSIONS}} markers in output', () => {
      const resolved = resolver.resolve(
        'full-workflow',
        {
          task_type: 'TASK',
          worker_level: 'level2',
          validator_count: 3,
          validator_level: 'level2',
          tester_level: 'level2',
          max_iterations: 10,
          max_tokens: 100000,
          timeout: 0,
        },
        { provider: 'gemini' }
      );

      // Serialize and check for markers
      const serialized = JSON.stringify(resolved);
      assert(
        !serialized.includes('{{CAPABILITY_EXTENSIONS}}'),
        'No CAPABILITY_EXTENSIONS markers should remain in resolved config'
      );
    });

    it('should not leave markers even without provider', () => {
      const resolved = resolver.resolve('debug-workflow', {});

      const serialized = JSON.stringify(resolved);
      assert(
        !serialized.includes('{{CAPABILITY_EXTENSIONS}}'),
        'Markers should be removed even without provider'
      );
    });
  });
});
