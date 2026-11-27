import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { act, createElement, isValidElement, type ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { assertDocument, findNode } from './domain';
import { flatType, freshType } from './schema';
import { readEnvironment } from './environment-store';
import { createHostFileAccess } from './run-history-source';
import type { RunFileAction } from './file-workflows';
import { createViteTestServer, runDetailFixture } from './test-support';
import {
  completePage,
  flushEffects,
  installBrowser,
  stubWorker,
  viewerModule,
} from './run-history-test-support';

const managed = {
  kind: 'managed' as const,
  target: 'selected-target',
  address: { scope: { kind: 'run' as const, runId: 'physical-part' }, path: 'report' },
};
const graph = () =>
  JSON.parse(
    readFileSync(
      new URL('../../docs/assets/file-workflow/file-workflow.graph.json', import.meta.url),
      'utf8'
    )
  );

test('filepath schemas and literal/bound publications survive saved graph editing; scalar graphs still open', async (t) => {
  const server = await createViteTestServer(t);
  const { PublicationsEditor } = await server.ssrLoadModule('/src/PublicationsEditor.tsx');
  const document = { name: 'files', graph: graph(), runtime: runDetailFixture().runtime };
  assertDocument(document);
  assert.ok(flatType(freshType('filepath')));
  assert.ok(flatType({ kind: 'array', items: freshType('filepath') }));
  let node = findNode(document.graph.root, 'done')!;
  function edit(match: (props: any) => boolean, change: (props: any) => void) {
    let selected: any;
    function visit(value: ReactNode) {
      if (Array.isArray(value)) return value.forEach(visit);
      if (!isValidElement<any>(value)) return;
      if (match(value.props)) selected = value.props;
      // ControlText's commit contract is sufficient here; mounted fields are covered separately.
      visit((value.props as { children?: ReactNode }).children);
    }
    function Exercise() {
      visit(
        PublicationsEditor({
          document,
          node,
          onChange: (next: any) => {
            node = next;
          },
        })
      );
      return null;
    }
    renderToStaticMarkup(createElement(Exercise));
    assert.ok(selected);
    change(selected);
  }
  const before = structuredClone(document);
  const html = renderToStaticMarkup(
    createElement(PublicationsEditor, {
      document,
      node,
      onChange: () => assert.fail('render changed graph'),
    })
  );
  assert.match(html, /state.*folder|folder/);
  assert.deepEqual(document, before);
  edit(
    (p) => p.value === JSON.stringify(node.artifacts.result.value) && p.onChange,
    (p) => p.onChange({ target: { value: 'literal' } })
  );
  edit(
    (p) => p.label === 'result path',
    (p) => p.onCommit('report/current')
  );
  assert.deepEqual(node.artifacts.result, { kind: 'literal', path: 'report/current' });
  edit(
    (p) => p.value === 'literal' && p.onChange,
    (p) => p.onChange({ target: { value: JSON.stringify({ source: 'state', path: ['folder'] }) } })
  );
  assert.deepEqual(node.artifacts.result, findNode(before.graph.root, 'done')!.artifacts.result);
  const request = findNode(document.graph.root, 'approval')!;
  assert.deepEqual(request.artifacts, { draft: { kind: 'literal', path: 'report' } });
  Object.assign(findNode(document.graph.root, 'done')!, node);
  const saved = JSON.parse(JSON.stringify(document));
  assertDocument(saved);
  assert.deepEqual(saved, before);
  const legacy = {
    name: 'legacy',
    graph: {
      profile: 'openengine.graph.full/v1',
      initialInput: { kind: 'string' },
      policy: {},
      root: { kind: 'succeed', name: 'done', output: { kind: 'string' }, bindings: [] },
    },
    runtime: runDetailFixture().runtime,
  };
  const original = JSON.stringify(legacy);
  assertDocument(legacy);
  assert.equal(JSON.stringify(legacy), original);
});

test('host attachment reads require explicit capability and retain encoded identity and cancellation', async () => {
  const calls: any[] = [];
  const api: any = async (...args: any[]) => {
    calls.push(args);
    return { artifacts: { draft: managed } };
  };
  const old = createHostFileAccess(api, { kind: 'openengine.workspace-files/v1' }, () => {});
  assert.equal(old.attachments, undefined);
  assert.deepEqual(old.actions, []);
  const access = createHostFileAccess(
    api,
    { kind: 'openengine.workspace-files/v1', requestAttachments: true, download: true },
    () => {}
  );
  const controller = new AbortController();
  assert.deepEqual(await access.attachments!('run/a', 'request/b', controller.signal), {
    draft: managed,
  });
  assert.deepEqual(calls, [
    ['runs/run%2Fa/requests/request%2Fb/files', undefined, controller.signal],
  ]);
  assert.deepEqual(access.actions, ['download']);
  const invalid = createHostFileAccess(
    (async () => ({ artifacts: { draft: { kind: 'url', url: 'https://other' } } })) as any,
    { kind: 'openengine.workspace-files/v1', requestAttachments: true },
    () => {}
  );
  await assert.rejects(invalid.attachments!('run', 'request', controller.signal));
});

test('mounted request and result views gate actions, clear authorization changes and distinguish live paths', async (t) => {
  let cleanup = async () => {};
  t.after(() => cleanup());
  const window = installBrowser(t);
  const server = await createViteTestServer(t);
  const { PublishedFiles, RequestAttachments, RunFilesProvider } =
    await server.ssrLoadModule('/src/PublishedFiles.tsx');
  const { createRoot } = await import('react-dom/client');
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  cleanup = async () => {
    await act(async () => root.unmount());
    container.remove();
  };
  const actions: RunFileAction[] = [];
  const reads: AbortSignal[] = [];
  const access = {
    actions: ['inspect', 'download'],
    attachments: async (_run: string, _request: string, signal: AbortSignal) => {
      reads.push(signal);
      return { draft: managed };
    },
    open: (action: RunFileAction) => {
      actions.push(action);
    },
  };
  async function mount(filesAccess?: any, request = true) {
    await act(async () =>
      root.render(
        createElement(
          RunFilesProvider,
          { runId: 'logical-run', access: filesAccess },
          request
            ? createElement(RequestAttachments, { runId: 'logical-run', requestId: 'question' })
            : createElement(PublishedFiles, {
                files: { result: { kind: 'local', root: '/local/work', path: 'report' } },
              })
        )
      )
    );
  }
  await mount();
  assert.equal(container.textContent, '', 'status-only viewer has no request metadata');
  await mount(access);
  assert.match(
    container.textContent!,
    /Retained publication.*report.*selected-target.*physical-part/
  );
  const inspect = [...container.querySelectorAll('button')].find(
    (b) => b.textContent === 'Inspect draft'
  )!;
  inspect.focus();
  assert.equal(document.activeElement, inspect);
  // Native buttons use the same click handler for keyboard activation (detail=0).
  await act(async () =>
    inspect.dispatchEvent(new window.MouseEvent('click', { bubbles: true, detail: 0 }))
  );
  assert.deepEqual(actions, [
    {
      runId: 'logical-run',
      requestId: 'question',
      name: 'draft',
      file: managed,
      action: 'inspect',
    },
  ]);
  await mount({});
  assert.equal(container.textContent, '', 'revoked metadata is not retained');
  assert.ok(reads.every((signal) => signal.aborted));
  await mount(undefined, false);
  assert.match(container.textContent!, /Live local path.*current contents.*\/local\/work\/report/);
  assert.equal(container.querySelectorAll('button').length, 0);
  assert.match(container.textContent!, /may change or be removed/);
});

test('shared run viewer displays a null result publication for standalone and capable hosts', async (t) => {
  let cleanup = async () => {};
  t.after(() => cleanup());
  stubWorker(t);
  installBrowser(t);
  const { RunHistoryView } = await viewerModule(t);
  const { createRoot } = await import('react-dom/client');
  const detail: any = {
    ...runDetailFixture('files-run'),
    phase: 'finished',
    terminal: { status: 'succeeded', output: null },
    metadata: { artifacts: { result: managed } },
    observation: { state: 'complete' },
  };
  const source: any = {
    detail: async () => detail,
    page: async () => ({
      ...completePage(),
      events: [
        { cursor: 'v2:1', event: { kind: 'run_started' } },
        { cursor: 'v2:2', event: { kind: 'terminal', result: detail.terminal } },
      ],
      nextCursor: 'v2:2',
      headCursor: 'v2:2',
    }),
  };
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  cleanup = async () => {
    await act(async () => root.unmount());
    container.remove();
  };
  await act(async () =>
    root.render(createElement(RunHistoryView, { runId: detail.runId, source, workers: [] }))
  );
  await flushEffects();
  const completed = [...container.querySelectorAll<HTMLButtonElement>('.story-step-head')].find(
    (button) => button.textContent?.includes('Completed')
  );
  assert.ok(completed);
  await act(async () => completed.click());
  assert.match(container.textContent!, /Retained publication/);
  assert.match(container.textContent!, /Client.download_file/);
  assert.equal(
    [...container.querySelectorAll('button')].some((b) => b.textContent === 'Download result'),
    false
  );
  const actions: any[] = [];
  const hosted = {
    ...source,
    files: { actions: ['download'], open: (value: any) => actions.push(value) },
  };
  await act(async () =>
    root.render(createElement(RunHistoryView, { runId: detail.runId, source: hosted, workers: [] }))
  );
  await flushEffects();
  const end = [...container.querySelectorAll<HTMLButtonElement>('.story-step-head')].find(
    (candidate) => candidate.textContent?.includes('Completed')
  )!;
  if (!container.querySelector('.published-files')) await act(async () => end.click());
  const button = [...container.querySelectorAll('button')].find(
    (b) => b.textContent === 'Download result'
  );
  assert.ok(button);
  await act(async () => button.click());
  assert.equal(actions[0].file.address.scope.runId, 'physical-part');
});

test('environment seeds round-trip independently from runtime and reject run-scoped seeds', () => {
  const environment = {
    id: 'env',
    name: 'files',
    revision: 'r1',
    definition: {
      startup: 'echo ready',
      seeds: [{ address: { scope: { kind: 'org' }, path: 'data' }, member: 'subset' }],
    },
  };
  assert.deepEqual(readEnvironment(JSON.parse(JSON.stringify(environment))), environment);
  assert.equal(
    readEnvironment({
      ...environment,
      definition: { seeds: [{ address: managed.address, member: '' }] },
    }),
    undefined
  );
  assert.equal(
    readEnvironment({
      ...environment,
      definition: { seeds: [{ address: { scope: { kind: ['user'] }, path: 'data' }, member: '' }] },
    }),
    undefined
  );
});
