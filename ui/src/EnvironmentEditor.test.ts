import test from 'node:test';
import assert from 'node:assert/strict';
import { createElement, isValidElement, type ReactElement, type ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createViteTestServer } from './test-support';

test('environment edits preserve connection declarations and omitted scripts stay omitted', async (t) => {
  const server = await createViteTestServer(t);
  const { EnvironmentEditor } = await server.ssrLoadModule('/src/EnvironmentEditor.tsx');
  let value: any = { connections: { registry: ['NPM_TOKEN'] }, variables: { NODE_ENV: 'test' } };
  const edits: string[] = [];
  const props = () => ({
    value,
    onChange: (next: any, key: string) => {
      value = next;
      edits.push(key);
    },
  });
  function interact(match: (node: ReactElement<any>) => boolean, action: (props: any) => void) {
    let target: ReactElement<any> | undefined;
    function visit(node: ReactNode) {
      if (Array.isArray(node)) return node.forEach(visit);
      if (!isValidElement<{ children?: ReactNode }>(node)) return;
      if (match(node)) target = node;
      if (typeof node.type === 'function') visit((node.type as any)(node.props));
      else visit(node.props.children);
    }
    function Exercise() {
      visit(EnvironmentEditor(props()));
      return null;
    }
    renderToStaticMarkup(createElement(Exercise));
    assert.ok(target, 'the requested environment field must exist');
    action(target.props);
  }
  const initial = structuredClone(value);
  const html = renderToStaticMarkup(createElement(EnvironmentEditor, props()));
  assert.match(html, /Setup script/);
  assert.match(html, /Startup script/);
  assert.match(html, /preserve existing workspace files on resume/);
  assert.match(html, /Use connections for secrets/);
  assert.deepEqual(value, initial);
  interact(
    (node) => node.type === 'textarea' && node.props.placeholder.startsWith('apt-get'),
    (field) => field.onChange({ target: { value: 'apt-get install -y make' } })
  );
  interact(
    (node) => node.type === 'textarea' && node.props.placeholder === 'npm ci',
    (field) => field.onChange({ target: { value: 'npm ci\n./scripts/start-services.sh' } })
  );
  interact(
    (node) => node.props['aria-label'] === 'Variable NODE_ENV value',
    (field) => field.onChange({ target: { value: 'development' } })
  );
  assert.deepEqual(value, {
    ...initial,
    variables: { NODE_ENV: 'development' },
    setup: 'apt-get install -y make',
    startup: 'npm ci\n./scripts/start-services.sh',
  });
  interact(
    (node) => node.props['aria-label'] === 'Remove variable NODE_ENV',
    (field) => field.onClick()
  );
  interact(
    (node) => node.type === 'textarea' && node.props.placeholder.startsWith('apt-get'),
    (field) => field.onChange({ target: { value: '' } })
  );
  assert.equal(Object.hasOwn(value, 'variables'), false);
  assert.equal(Object.hasOwn(value, 'setup'), false);
  assert.deepEqual(value.connections, initial.connections);
  assert.deepEqual(edits, ['setup', 'startup', 'variables', 'variables', 'setup']);
  interact(
    (node) => node.props.label === 'Preparation connection registry fields',
    (field) => field.onCommit('NPM_TOKEN, REGISTRY_TOKEN')
  );
  interact(
    (node) => node.props.label === 'Preparation connection registry name',
    (field) => field.onCommit('private_registry')
  );
  assert.deepEqual(value.connections, { private_registry: ['NPM_TOKEN', 'REGISTRY_TOKEN'] });
  assert.equal(value.startup, 'npm ci\n./scripts/start-services.sh');
  interact(
    (node) => node.props['aria-label'] === 'Remove preparation connection private_registry',
    (field) => field.onClick()
  );
  assert.equal(Object.hasOwn(value, 'connections'), false);
});
