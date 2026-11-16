import test, { type TestContext } from 'node:test';
import assert from 'node:assert/strict';
import { act, createElement, useState } from 'react';
import { JSDOM } from 'jsdom';
import { createViteTestServer, installTestGlobals } from './test-support';

async function mount(t: TestContext, render: (modules: Record<string, any>) => unknown) {
  const dom = new JSDOM(
    '<!doctype html><html><body><button>Outside</button><div id="root"></div></body></html>',
    { pretendToBeVisual: true, url: 'https://example.test/ui/' }
  );
  const restore = installTestGlobals({
    window: dom.window,
    document: dom.window.document,
    Node: dom.window.Node,
    HTMLElement: dom.window.HTMLElement,
    IS_REACT_ACT_ENVIRONMENT: true,
  });
  const server = await createViteTestServer(t);
  const modules = {
    ...(await server.ssrLoadModule('/src/MoreMenu.tsx')),
    ...(await server.ssrLoadModule('/src/AppHeader.tsx')),
  };
  const { createRoot } = await import('react-dom/client');
  const document = dom.window.document;
  const root = createRoot(document.getElementById('root')!);
  await act(async () => root.render(render(modules) as any));
  t.after(async () => {
    await act(async () => root.unmount());
    restore();
    dom.window.close();
  });
  const key = async (key: string) =>
    act(async () => {
      document.activeElement!.dispatchEvent(
        new dom.window.KeyboardEvent('keydown', { key, bubbles: true })
      );
    });
  const byLabel = (label: string) =>
    document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;
  return { document, key, byLabel, outside: document.querySelector('body > button')! };
}

function renderMenu(t: TestContext, focusWhenSelected: (Element | null)[]) {
  const item = (label: string, disabled = false) => ({
    label,
    icon: null,
    disabled,
    onSelect: () => focusWhenSelected.push(globalThis.document.activeElement),
  });
  return mount(t, ({ MoreMenu }) =>
    createElement(MoreMenu, {
      label: 'More tools',
      items: [item('First'), item('Unavailable', true), item('Last')],
    })
  );
}

test('the More menu moves between its items with the arrow, Home and End keys', async (t) => {
  const page = await renderMenu(t, []);
  const trigger = page.byLabel('More tools');
  await act(async () => trigger.click());
  const items = [...page.document.querySelectorAll('[role="menuitem"]')];
  assert.equal(trigger.getAttribute('aria-expanded'), 'true');
  assert.equal(
    trigger.getAttribute('aria-controls'),
    page.document.querySelector('[role="menu"]')!.id
  );
  assert.equal(page.document.activeElement, items[0]);
  await page.key('ArrowDown');
  assert.equal(page.document.activeElement, items[1]);
  await page.key('End');
  assert.equal(page.document.activeElement, items[2]);
  await page.key('ArrowDown');
  assert.equal(page.document.activeElement, items[0]);
  await page.key('ArrowUp');
  assert.equal(page.document.activeElement, items[2]);
  await page.key('Home');
  assert.equal(page.document.activeElement, items[0]);
});

test('choosing a More menu item returns focus to its button before the item acts', async (t) => {
  const focused: (Element | null)[] = [];
  const page = await renderMenu(t, focused);
  const trigger = page.byLabel('More tools');
  await act(async () => trigger.click());
  const [, unavailable, last] = page.document.querySelectorAll<HTMLElement>('[role="menuitem"]');
  await act(async () => unavailable.click());
  assert.deepEqual(focused, []);
  assert.ok(page.document.querySelector('[role="menu"]'));
  await act(async () => last.click());
  // A dialog opened by the item records this as its opener and returns focus here.
  assert.deepEqual(focused, [trigger]);
  assert.equal(page.document.querySelector('[role="menu"]'), null);
  assert.equal(page.document.activeElement, trigger);
});

test('the More menu closes on Escape or when focus leaves it', async (t) => {
  const page = await renderMenu(t, []);
  const trigger = page.byLabel('More tools');
  await act(async () => trigger.click());
  await page.key('Escape');
  assert.equal(page.document.querySelector('[role="menu"]'), null);
  assert.equal(page.document.activeElement, trigger);

  await act(async () => trigger.click());
  assert.ok(page.document.querySelector('[role="menu"]'));
  await act(async () => (page.outside as HTMLElement).focus());
  assert.equal(page.document.querySelector('[role="menu"]'), null);
});

test('Escape inside an overlaid sidebar closes it and returns focus to its toggle', async (t) => {
  const page = await mount(t, ({ AppHeader }) => {
    function Page() {
      const [open, setOpen] = useState(false);
      return createElement(
        'div',
        null,
        createElement(AppHeader, {
          section: 'profiles',
          navigate: () => {},
          workspace: { kind: 'local', id: 'test' },
          sidebar: { label: 'Profiles', id: 'profiles-sidebar', open, setOpen },
        }),
        createElement(
          'nav',
          { id: 'profiles-sidebar', hidden: !open },
          createElement('button', null, 'A profile')
        )
      );
    }
    return createElement(Page);
  });
  const toggle = page.byLabel('Profiles');
  assert.equal(toggle.getAttribute('aria-controls'), 'profiles-sidebar');
  assert.equal(toggle.getAttribute('aria-expanded'), 'false');
  await act(async () => toggle.click());
  assert.equal(toggle.getAttribute('aria-expanded'), 'true');
  const profile = page.document.querySelector<HTMLElement>('#profiles-sidebar button')!;
  await act(async () => profile.focus());
  await page.key('Escape');
  assert.equal(toggle.getAttribute('aria-expanded'), 'false');
  assert.equal(page.document.activeElement, toggle);
});
