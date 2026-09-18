import test from 'node:test';
import assert from 'node:assert/strict';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { ModelPicker } from './ModelPicker';
import { suggestedModels } from './models';

test('suggestions retain the provider-specific Astra and Fable wire IDs', () => {
  assert.ok(suggestedModels('codex', 'openai').includes('gpt-6-astra'));
  assert.ok(suggestedModels('codex', 'openrouter').includes('openai/gpt-6-astra'));
  assert.ok(suggestedModels('codex', 'bedrock').includes('openai.gpt-6-astra'));
  assert.ok(suggestedModels('claude', 'anthropic').includes('claude-fable-5-1'));
  assert.ok(suggestedModels('claude', 'openrouter').includes('anthropic/claude-fable-5.1'));
  assert.ok(suggestedModels('claude', 'bedrock').includes('global.anthropic.claude-fable-5-1'));
  assert.ok(!suggestedModels('claude', 'anthropic').includes('claude-fable-5.1'));
  assert.ok(!suggestedModels('claude', 'openrouter').includes('anthropic/claude-fable-5-1'));
});

test('unknown and incompatible lanes do not infer suggestions or use inherited object keys', () => {
  for (const [harness, provider] of [
    ['codex', 'anthropic'],
    ['claude', 'openai'],
    ['future-harness', 'openai'],
    ['codex', 'future-provider'],
    ['__proto__', 'openai'],
    ['codex', 'constructor'],
    ['toString', 'toString'],
  ]) {
    assert.deepEqual(suggestedModels(harness, provider), []);
  }
});

test('model picker preserves authored custom values with and without suggestions', () => {
  const changes: string[] = [];
  for (const [harness, provider] of [
    ['codex', 'openai'],
    ['claude', 'anthropic'],
    ['future-harness', 'private-provider'],
  ]) {
    const markup = renderToStaticMarkup(
      createElement(ModelPicker, {
        label: 'Model',
        value: 'private/model-release:custom',
        harness,
        provider,
        onChange: (value) => changes.push(value),
      })
    );
    assert.match(markup, /value="private\/model-release:custom"/);
    assert.equal(markup.includes('<select'), harness !== 'future-harness');
  }
  assert.deepEqual(changes, []);
});
