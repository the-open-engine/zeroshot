import test from 'node:test';
import assert from 'node:assert/strict';
import { act, createElement } from 'react';
import { createViteTestServer, runDetailFixture } from './test-support';
import { completePage, flushEffects, installBrowser, stubWorker } from './run-history-test-support';

for (const capable of [false, true]) {
  test(`embedded file actions ${capable ? 'reach a capable host' : 'keep the old host fallback'}`, async (t) => {
    let cleanup = async () => {};
    t.after(() => cleanup());
    stubWorker(t);
    const browser = installBrowser(t);
    const messages: any[] = [];
    const parent = {
      postMessage: (value: any, origin: string) => {
        assert.equal(origin, browser.location.origin);
        messages.push(value);
      },
    };
    Object.defineProperty(browser, 'parent', { configurable: true, value: parent });
    const detail: any = {
      ...runDetailFixture('selected-run'),
      phase: 'finished',
      observation: { state: 'complete' },
      terminal: { status: 'succeeded', output: null },
      metadata: {
        artifacts: {
          result: {
            kind: 'managed',
            target: 'selected',
            address: { scope: { kind: 'run', runId: 'producing-part' }, path: 'report' },
          },
        },
      },
    };
    const reads: string[] = [];
    const fetcher = t.mock.method(globalThis, 'fetch', async (input: any) => {
      const url = new URL(String(input));
      assert.equal(url.origin, browser.location.origin);
      reads.push(url.pathname);
      if (url.pathname === '/host/bootstrap')
        return Response.json({
          version: 1,
          workspace: { kind: 'platform', id: 'scope' },
          templates: [],
          runtimeSchema: {},
        });
      if (url.pathname === '/host/profiles') return Response.json({ profiles: [] });
      if (url.pathname === '/host/runs/selected-run') return Response.json(detail);
      if (url.pathname === '/host/runs/selected-run/history')
        return Response.json({
          ...completePage(),
          events: [
            { cursor: 'v2:1', event: { kind: 'run_started' } },
            { cursor: 'v2:2', event: { kind: 'terminal', result: detail.terminal } },
          ],
          nextCursor: 'v2:2',
          headCursor: 'v2:2',
        });
      assert.fail(`Unexpected host read: ${url.pathname}`);
    });
    const server = await createViteTestServer(t);
    const { EmbeddedApp } = await server.ssrLoadModule('/src/EmbeddedApp.tsx');
    const { createRoot } = await import('react-dom/client');
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    cleanup = async () => {
      await act(async () => root.unmount());
      container.remove();
      fetcher.mock.restore();
    };
    await act(async () => root.render(createElement(EmbeddedApp)));
    const send = async (data: any) =>
      act(async () => {
        browser.dispatchEvent(
          new browser.MessageEvent('message', {
            source: parent as any,
            origin: browser.location.origin,
            data,
          })
        );
      });
    await send({
      version: 1,
      type: 'init',
      requestId: messages[0].requestId,
      workspaceId: 'scope',
      authority: { userId: 'user', organizationId: 'org', scope: 'user' },
      apiBase: '/host/',
      theme: 'light',
      ...(capable && { files: { kind: 'openengine.workspace-files/v1', download: true } }),
    });
    await flushEffects();
    const state = messages.filter((m) => m.type === 'state').at(-1);
    assert.ok(state, container.textContent!);
    await send({
      ...state,
      type: 'open_run',
      requestId: 'open-run',
      nextDocumentId: 'run-document',
      runId: detail.runId,
    });
    await flushEffects();
    const completed = [...container.querySelectorAll<HTMLButtonElement>('.story-step-head')].find(
      (button) => button.textContent?.includes('Completed')
    );
    assert.ok(completed, container.textContent!);
    await act(async () => completed.click());
    assert.match(container.textContent!, /Retained publication/);
    const download = [...container.querySelectorAll<HTMLButtonElement>('button')].find(
      (button) => button.textContent === 'Download result'
    );
    if (capable) {
      assert.ok(download);
      download.focus();
      await act(async () =>
        download.dispatchEvent(new browser.MouseEvent('click', { bubbles: true, detail: 0 }))
      );
      const message = messages.find((m) => m.type === 'file_action');
      assert.equal(message.documentId, 'run-document');
      assert.equal(message.selection.file.address.scope.runId, 'producing-part');
      assert.equal(message.selection.action, 'download');
    } else {
      assert.equal(download, undefined);
      assert.match(container.textContent!, /Client.download_file/);
      assert.equal(
        messages.some((m) => m.type === 'file_action'),
        false
      );
    }
    assert.ok(reads.every((path) => !path.includes('/files')));
  });
}
