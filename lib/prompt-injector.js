/**
 * PromptInjector - Injects capability-specific prompt extensions
 *
 * Couples to CAPABILITIES, not provider names. Loads and merges
 * capability extensions based on what the provider can actually do.
 *
 * Usage:
 *   const prompt = injectCapabilityExtensions(basePrompt, 'claude');
 *   // Result: basePrompt + sub-agent instructions + parallel execution guide
 */

const fs = require('fs');
const path = require('path');
const { getCapabilities } = require('../src/providers/capabilities');

class PromptInjector {
  /**
   * @param {string} extensionsDir - Path to capability-extensions directory
   */
  constructor(extensionsDir) {
    this.extensionsDir = extensionsDir;
    this.cache = new Map();
  }

  /**
   * Inject capability-specific extensions into a prompt
   * @param {string} basePrompt - Base prompt text (provider-agnostic)
   * @param {string} provider - Provider name (claude, openai, codex, gemini)
   * @returns {string} Final prompt with capability extensions injected
   */
  injectCapabilityExtensions(basePrompt, provider) {
    const caps = getCapabilities(provider);
    const extensions = [];

    // Load capability extensions based on what this provider supports
    if (caps.subAgents) {
      extensions.push(this._loadExtension('sub-agents.md'));
    }

    if (caps.parallelToolCalls) {
      extensions.push(this._loadExtension('parallel-execution.md'));
    }

    // If there are extensions to inject, append them
    if (extensions.length === 0) {
      return basePrompt;
    }

    return basePrompt + '\n\n' + extensions.join('\n\n');
  }

  /**
   * Inject extensions at a specific marker in the prompt
   * @param {string} promptWithMarker - Prompt containing {{CAPABILITY_EXTENSIONS}} marker
   * @param {string} provider - Provider name
   * @returns {string} Prompt with marker replaced by capability extensions
   */
  injectAtMarker(promptWithMarker, provider) {
    const marker = '{{CAPABILITY_EXTENSIONS}}';

    if (!promptWithMarker.includes(marker)) {
      // No marker, just append (backward compatibility)
      return this.injectCapabilityExtensions(promptWithMarker, provider);
    }

    const caps = getCapabilities(provider);
    const extensions = [];

    if (caps.subAgents) {
      extensions.push(this._loadExtension('sub-agents.md'));
    }

    if (caps.parallelToolCalls) {
      extensions.push(this._loadExtension('parallel-execution.md'));
    }

    const extensionText = extensions.length > 0 ? extensions.join('\n\n') : '';
    return promptWithMarker.replace(marker, extensionText);
  }

  /**
   * Load a capability extension file (with caching)
   * @private
   * @param {string} filename - Extension filename (e.g., 'sub-agents.md')
   * @returns {string} Extension content
   */
  _loadExtension(filename) {
    if (this.cache.has(filename)) {
      return this.cache.get(filename);
    }

    const filepath = path.join(this.extensionsDir, filename);

    if (!fs.existsSync(filepath)) {
      console.warn(`Capability extension not found: ${filename} (looked in ${filepath})`);
      return '';
    }

    const content = fs.readFileSync(filepath, 'utf8').trim();
    this.cache.set(filename, content);
    return content;
  }

  /**
   * Clear the extension cache (useful for testing)
   */
  clearCache() {
    this.cache.clear();
  }
}

// Convenience function for single-use injection
function injectCapabilityExtensions(basePrompt, provider, extensionsDir) {
  const injector = new PromptInjector(extensionsDir);
  return injector.injectCapabilityExtensions(basePrompt, provider);
}

// Convenience function for marker-based injection
function injectAtMarker(promptWithMarker, provider, extensionsDir) {
  const injector = new PromptInjector(extensionsDir);
  return injector.injectAtMarker(promptWithMarker, provider);
}

module.exports = {
  PromptInjector,
  injectCapabilityExtensions,
  injectAtMarker,
};
