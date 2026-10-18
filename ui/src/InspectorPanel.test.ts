import test, { type TestContext } from 'node:test';
import assert from 'node:assert/strict';
import { act, createElement, Fragment } from 'react';
import { JSDOM } from 'jsdom';
import { createViteTestServer, installTestGlobals } from './test-support';

async function inspectorPage(t: TestContext) {
  const dom = new JSDOM(
    '<!doctype html><style>.inspector-panel { position: relative; --inspector-default-width: 322px; } .overlay .inspector-panel { position: absolute; }</style><div id="root"></div>',
    { pretendToBeVisual: true, url: 'https://example.test/ui/' }
  );
  let available = 1200;
  let measure = () => {};
  let captured: number | undefined;
  const globals = {
    window: dom.window,
    document: dom.window.document,
    IS_REACT_ACT_ENVIRONMENT: true,
    ResizeObserver: class {
      constructor(callback: () => void) {
        measure = callback;
      }
      observe() {}
      disconnect() {}
    },
  };
  const restoreGlobals = installTestGlobals(globals);
  dom.window.HTMLElement.prototype.setPointerCapture = (id) => {
    captured = id;
  };
  dom.window.HTMLElement.prototype.releasePointerCapture = (id) => {
    assert.equal(captured, id);
    captured = undefined;
  };
  const host = dom.window.document.getElementById('root')!;
  host.getBoundingClientRect = () => ({ width: available }) as DOMRect;
  Object.defineProperty(dom.window, 'innerWidth', { value: 1600, configurable: true });
  const server = await createViteTestServer(t);
  const { InspectorPanel } = await server.ssrLoadModule('/src/InspectorPanel.tsx');
  const { createRoot } = await import('react-dom/client');
  const root = createRoot(host);
  const render = async (run = false) => {
    await act(async () =>
      root.render(
        createElement(
          Fragment,
          null,
          createElement(
            InspectorPanel,
            { key: 'profile', className: 'inspector', label: 'Profile inspector' },
            'Definition'
          ),
          run &&
            createElement(
              InspectorPanel,
              { key: 'run', className: 'run-inspector', label: 'Run inspector' },
              'JSON'
            )
        )
      )
    );
  };
  await render();
  t.after(async () => {
    await act(async () => root.unmount());
    restoreGlobals();
    dom.window.close();
  });
  const separator = () => host.querySelector<HTMLElement>('[role="separator"]')!;
  return {
    host,
    render,
    separator,
    width: () => Number(separator().getAttribute('aria-valuenow')),
    stored: () => dom.window.localStorage.getItem('zeroshot.inspector-width.v1'),
    captured: () => captured,
    async key(key: string, shiftKey = false) {
      await act(async () =>
        separator().dispatchEvent(
          new dom.window.KeyboardEvent('keydown', { bubbles: true, key, shiftKey })
        )
      );
    },
    async pointer(type: string, clientX: number) {
      await act(async () =>
        separator().dispatchEvent(
          new dom.window.PointerEvent(type, { bubbles: true, pointerId: 7, button: 0, clientX })
        )
      );
    },
    async resize(width: number, overlay = false) {
      available = width;
      host.classList.toggle('overlay', overlay);
      await act(async () => measure());
    },
  };
}

test('the shared inspector separator supports keyboard bounds and keeps both inspectors coherent', async (t) => {
  const page = await inspectorPage(t);
  assert.equal(page.separator().getAttribute('aria-label'), 'Resize profile inspector');
  assert.equal(page.separator().getAttribute('aria-orientation'), 'vertical');
  assert.equal(page.width(), 322);
  await page.key('ArrowLeft');
  assert.equal(page.width(), 338);
  await page.key('ArrowRight', true);
  assert.equal(page.width(), 280);
  await page.key('End');
  assert.equal(page.width(), 840);
  await page.key('ArrowLeft');
  assert.equal(page.width(), 840);
  await page.render(true);
  const widths = () =>
    Array.from(page.host.querySelectorAll('[role="separator"]'), (node) =>
      node.getAttribute('aria-valuenow')
    );
  assert.deepEqual(widths(), ['840', '840']);
  await page.key('Home');
  assert.deepEqual(widths(), ['280', '280']);
  assert.equal(page.stored(), '280');
});

test('pointer dragging grows the right inspector to the left and cancellation restores its width', async (t) => {
  const page = await inspectorPage(t);
  await page.pointer('pointerdown', 900);
  assert.equal(page.captured(), 7);
  assert.equal(page.host.ownerDocument.activeElement, page.separator());
  await page.pointer('pointermove', 640);
  assert.equal(page.width(), 582);
  assert.equal(page.stored(), null, 'a drag persists only once it finishes');
  await page.pointer('pointerup', 640);
  assert.equal(page.stored(), '582');
  assert.equal(page.captured(), undefined);
  await page.pointer('pointerdown', 640);
  await page.pointer('pointermove', 450);
  assert.equal(page.width(), 772);
  await page.pointer('pointercancel', 450);
  assert.equal(page.width(), 582);
  assert.equal(page.stored(), '582');
});

test('narrow overlays keep a canvas edge visible and restore the preferred width when space returns', async (t) => {
  const page = await inspectorPage(t);
  await page.key('End');
  assert.equal(page.width(), 840);
  await page.resize(390, true);
  assert.equal(page.width(), 366);
  assert.equal(page.separator().getAttribute('aria-valuemax'), '366');
  assert.equal(page.stored(), '840');
  await page.resize(240, true);
  assert.equal(page.width(), 216);
  assert.equal(page.separator().getAttribute('aria-valuemin'), '216');
  await page.resize(1200);
  assert.equal(page.width(), 840);
  await page.key('Home');
  assert.equal(page.width(), 280);
});
