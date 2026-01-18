const assert = require('assert');
const fs = require('fs');
const path = require('path');
const os = require('os');
const {
  PromptInjector,
  injectCapabilityExtensions,
  injectAtMarker,
} = require('../lib/prompt-injector');

describe('PromptInjector', () => {
  let tempDir;
  let extensionsDir;
  let injector;

  beforeEach(() => {
    // Create temp directory for test extensions
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'prompt-injector-test-'));
    extensionsDir = path.join(tempDir, 'capability-extensions');
    fs.mkdirSync(extensionsDir);

    // Create test extension files
    fs.writeFileSync(
      path.join(extensionsDir, 'sub-agents.md'),
      '## SUB-AGENT INSTRUCTIONS\nUse Task tool for delegation.'
    );
    fs.writeFileSync(
      path.join(extensionsDir, 'parallel-execution.md'),
      '## PARALLEL EXECUTION\nCall multiple tools simultaneously.'
    );

    injector = new PromptInjector(extensionsDir);
  });

  afterEach(() => {
    // Cleanup temp directory
    if (tempDir && fs.existsSync(tempDir)) {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  describe('injectCapabilityExtensions', () => {
    it('should inject sub-agent instructions for Claude', () => {
      const basePrompt = 'You are an agent.\n\nDo your work.';
      const result = injector.injectCapabilityExtensions(basePrompt, 'claude');

      assert.ok(result.includes('You are an agent'), 'Should preserve base prompt');
      assert.ok(result.includes('SUB-AGENT INSTRUCTIONS'), 'Should include sub-agent extension');
      assert.ok(result.includes('Task tool'), 'Should include Task tool reference');
      assert.ok(
        result.includes('PARALLEL EXECUTION'),
        'Should include parallel execution extension'
      );
    });

    it('should NOT inject sub-agent instructions for Gemini', () => {
      const basePrompt = 'You are an agent.\n\nDo your work.';
      const result = injector.injectCapabilityExtensions(basePrompt, 'gemini');

      assert.ok(result.includes('You are an agent'), 'Should preserve base prompt');
      assert.ok(
        !result.includes('SUB-AGENT INSTRUCTIONS'),
        'Should NOT include sub-agent extension'
      );
      assert.ok(!result.includes('Task tool'), 'Should NOT include Task tool reference');
      assert.ok(
        result.includes('PARALLEL EXECUTION'),
        'Should include parallel execution extension'
      );
    });

    it('should NOT inject sub-agent instructions for Codex', () => {
      const basePrompt = 'You are an agent.\n\nDo your work.';
      const result = injector.injectCapabilityExtensions(basePrompt, 'codex');

      assert.ok(result.includes('You are an agent'), 'Should preserve base prompt');
      assert.ok(
        !result.includes('SUB-AGENT INSTRUCTIONS'),
        'Should NOT include sub-agent extension'
      );
      assert.ok(!result.includes('Task tool'), 'Should NOT include Task tool reference');
      assert.ok(!result.includes('PARALLEL EXECUTION'), 'Codex does not support parallel tools');
    });

    it('should inject parallel execution for Gemini', () => {
      const basePrompt = 'You are an agent.\n\nDo your work.';
      const result = injector.injectCapabilityExtensions(basePrompt, 'gemini');

      assert.ok(result.includes('You are an agent'), 'Should preserve base prompt');
      assert.ok(
        !result.includes('SUB-AGENT INSTRUCTIONS'),
        'Should NOT include sub-agent extension'
      );
      assert.ok(
        result.includes('PARALLEL EXECUTION'),
        'Should include parallel execution extension'
      );
    });

    it('should handle unknown provider with minimal capabilities', () => {
      const basePrompt = 'You are an agent.\n\nDo your work.';
      const result = injector.injectCapabilityExtensions(basePrompt, 'unknown-provider');

      assert.strictEqual(result, basePrompt, 'Should return unchanged prompt for unknown provider');
      assert.ok(!result.includes('SUB-AGENT INSTRUCTIONS'), 'Should NOT inject any extensions');
      assert.ok(!result.includes('PARALLEL EXECUTION'), 'Should NOT inject any extensions');
    });

    it('should return base prompt when no extensions match', () => {
      const basePrompt = 'You are an agent.\n\nDo your work.';
      const result = injector.injectCapabilityExtensions(basePrompt, 'codex');

      assert.strictEqual(result, basePrompt, 'Should return unchanged when no capabilities match');
    });
  });

  describe('injectAtMarker', () => {
    it('should replace {{CAPABILITY_EXTENSIONS}} marker with extensions', () => {
      const promptWithMarker = 'Base prompt.\n\n{{CAPABILITY_EXTENSIONS}}\n\nMore instructions.';
      const result = injector.injectAtMarker(promptWithMarker, 'claude');

      assert.ok(result.includes('Base prompt'), 'Should preserve content before marker');
      assert.ok(result.includes('More instructions'), 'Should preserve content after marker');
      assert.ok(result.includes('SUB-AGENT INSTRUCTIONS'), 'Should inject extensions at marker');
      assert.ok(!result.includes('{{CAPABILITY_EXTENSIONS}}'), 'Should remove marker');
    });

    it('should replace marker with empty string when no capabilities match', () => {
      const promptWithMarker = 'Base prompt.\n\n{{CAPABILITY_EXTENSIONS}}\n\nMore instructions.';
      const result = injector.injectAtMarker(promptWithMarker, 'codex');

      assert.ok(result.includes('Base prompt'), 'Should preserve content');
      assert.ok(result.includes('More instructions'), 'Should preserve content');
      assert.ok(!result.includes('{{CAPABILITY_EXTENSIONS}}'), 'Should remove marker');
      assert.ok(!result.includes('SUB-AGENT'), 'Should not inject extensions');
    });

    it('should fall back to append mode if no marker present', () => {
      const promptWithoutMarker = 'Base prompt.\n\nDo your work.';
      const result = injector.injectAtMarker(promptWithoutMarker, 'claude');

      assert.ok(result.includes('Base prompt'), 'Should preserve base prompt');
      assert.ok(result.includes('SUB-AGENT INSTRUCTIONS'), 'Should append extensions');
      assert.ok(
        result.indexOf('Do your work') < result.indexOf('SUB-AGENT'),
        'Should append after base'
      );
    });
  });

  describe('extension file loading', () => {
    it('should cache loaded extensions', () => {
      const prompt1 = injector.injectCapabilityExtensions('Test 1', 'claude');
      const prompt2 = injector.injectCapabilityExtensions('Test 2', 'claude');

      assert.ok(prompt1.includes('SUB-AGENT INSTRUCTIONS'), 'First call should load extension');
      assert.ok(
        prompt2.includes('SUB-AGENT INSTRUCTIONS'),
        'Second call should use cached extension'
      );
      assert.strictEqual(injector.cache.size, 2, 'Should cache both extensions');
    });

    it('should warn and skip missing extension files', () => {
      // Remove one extension file
      fs.unlinkSync(path.join(extensionsDir, 'parallel-execution.md'));

      const basePrompt = 'You are an agent.';
      const result = injector.injectCapabilityExtensions(basePrompt, 'claude');

      // Should still work with available extension
      assert.ok(result.includes('SUB-AGENT INSTRUCTIONS'), 'Should include available extension');
      assert.ok(!result.includes('PARALLEL EXECUTION'), 'Should skip missing extension');
    });

    it('should clear cache when requested', () => {
      injector.injectCapabilityExtensions('Test', 'claude');
      assert.strictEqual(injector.cache.size, 2, 'Should cache extensions');

      injector.clearCache();
      assert.strictEqual(injector.cache.size, 0, 'Should clear cache');
    });
  });

  describe('convenience functions', () => {
    it('injectCapabilityExtensions should work as standalone function', () => {
      const basePrompt = 'You are an agent.';
      const result = injectCapabilityExtensions(basePrompt, 'claude', extensionsDir);

      assert.ok(result.includes('You are an agent'), 'Should work as standalone');
      assert.ok(result.includes('SUB-AGENT INSTRUCTIONS'), 'Should inject extensions');
    });

    it('injectAtMarker should work as standalone function', () => {
      const promptWithMarker = 'Base.\n\n{{CAPABILITY_EXTENSIONS}}\n\nEnd.';
      const result = injectAtMarker(promptWithMarker, 'claude', extensionsDir);

      assert.ok(result.includes('Base'), 'Should work as standalone');
      assert.ok(result.includes('SUB-AGENT INSTRUCTIONS'), 'Should inject at marker');
      assert.ok(!result.includes('{{CAPABILITY_EXTENSIONS}}'), 'Should remove marker');
    });
  });
});
