import test from 'node:test';
import assert from 'node:assert/strict';
import { act, createElement } from 'react';
import { JSDOM } from 'jsdom';
import { createViteTestServer, installTestGlobals } from './test-support';

test('decision controls preserve incomplete probability text across navigation and commit only valid probabilities', async (t) => {
  const dom = new JSDOM('<!doctype html><div id="root"></div>');
  const restore = installTestGlobals({
    window: dom.window,
    document: dom.window.document,
    HTMLElement: dom.window.HTMLElement,
    IS_REACT_ACT_ENVIRONMENT: true,
  });
  const server = await createViteTestServer(t);
  const { DecisionControls } = await server.ssrLoadModule('/src/DecisionControls.tsx');
  const { createNumericDrafts, NumericDraftProvider } =
    await server.ssrLoadModule('/src/numeric-drafts.ts');
  const { hasPendingEdits } = await server.ssrLoadModule('/src/pending-edits.ts');
  const drafts = createNumericDrafts();
  const { createRoot } = await import('react-dom/client');
  const root = createRoot(dom.window.document.getElementById('root')!);
  const binding = {
    kind: 'decision',
    model: 'caller/model',
    minProbability: 0.85,
    uncertainLabel: 'uncertain',
  };
  const writes: Array<[string, string | number]> = [];
  const mount = async (visible = true) =>
    act(async () =>
      root.render(
        createElement(
          NumericDraftProvider,
          { value: drafts },
          visible
            ? createElement(DecisionControls, {
                node: 'decide',
                binding,
                onChange: (field: string, value: string | number) => {
                  writes.push([field, value]);
                  Object.assign(binding, { [field]: value });
                },
              })
            : null
        )
      )
    );
  t.after(async () => {
    await act(async () => root.unmount());
    drafts.clear();
    dom.window.close();
    restore();
  });
  const field = (name: string) => {
    const label = [...dom.window.document.querySelectorAll('label')].find(
      (label) => label.textContent === name
    )!;
    return dom.window.document.getElementById(label.htmlFor) as HTMLInputElement;
  };
  const type = async (text: string) =>
    act(async () => {
      const input = field('Minimum probability');
      Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, 'value')!.set!.call(
        input,
        text
      );
      input.dispatchEvent(new dom.window.Event('input', { bubbles: true }));
    });
  const enter = async () =>
    act(async () => {
      field('Minimum probability').dispatchEvent(
        new dom.window.KeyboardEvent('keydown', { key: 'Enter', bubbles: true })
      );
    });
  await mount();
  assert.equal(field('Uncertain signal label').readOnly, true);
  await type('1e');
  await enter();
  assert.deepEqual(writes, []);
  assert.equal(binding.minProbability, 0.85);
  assert.equal(hasPendingEdits(), true);
  await mount(false);
  await mount();
  assert.equal(field('Minimum probability').value, '1e');
  for (const text of ['2', '-0.1', '']) {
    await type(text);
    await enter();
    assert.deepEqual(writes, [], text);
    assert.equal(field('Minimum probability').getAttribute('aria-invalid'), 'true');
  }
  await type('.7');
  await enter();
  assert.deepEqual(writes, [['minProbability', 0.7]]);
  assert.equal(binding.minProbability, 0.7);
  assert.equal(hasPendingEdits(), false);
});
