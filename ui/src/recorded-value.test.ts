import test from 'node:test';
import assert from 'node:assert/strict';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';
import { fileURLToPath } from 'node:url';
import {
  decodeRecordedValue,
  recordedFieldPath,
  recordedTextRange,
  RECORDED_TEXT_PAGE_SIZE,
} from './recorded-value';

// Preserve the distinction between ordinary prose and a fully parsed JSON container.
test('JSON decoding is strict, bounded, and preserves unsafe numeric or malformed text', () => {
  assert.deepEqual(decodeRecordedValue(' {"file":{"path":"plan.md"},"accepted":false} '), {
    file: { path: 'plan.md' },
    accepted: false,
  });
  assert.deepEqual(decodeRecordedValue('[null, false, 0, ""]'), [null, false, 0, '']);
  for (const source of [
    'false',
    '0',
    'null',
    '"Quoted prose"',
    'Notes {"path":"plan.md"}',
    '{"broken":}',
    '```json\n{"path":"plan.md"}\n```',
    '{"id":9007199254740993}',
    '[1e999]',
    '{"body":"' + 'x'.repeat(65536) + '"}',
  ]) {
    assert.equal(decodeRecordedValue(source), source);
  }
  const prototype = decodeRecordedValue('{"__proto__":{"polluted":true}}') as Record<
    string,
    unknown
  >;
  assert.equal(Object.hasOwn(prototype, '__proto__'), true);
  assert.equal(Object.hasOwn(Object.prototype, 'polluted'), false);
});

test('single-field paths flatten wrappers but preserve sibling groups, arrays and empty containers', () => {
  const value = { file: { path: 'plan.md' } };
  const flattened = recordedFieldPath('result', value);
  assert.deepEqual(flattened.path, ['result', 'file', 'path']);
  assert.equal(flattened.value, 'plan.md');
  const siblings = { path: 'plan.md', checksum: 'abc' };
  assert.deepEqual(recordedFieldPath('result', { file: siblings }).path, ['result', 'file']);
  assert.equal(recordedFieldPath('result', { file: siblings }).value, siblings);
  const array = [0, false, null];
  assert.equal(recordedFieldPath('result', { items: array }).value, array);
  assert.deepEqual(recordedFieldPath('empty', {}).path, ['empty']);
});

test('deep paths and cycles are bounded without mutating the source', () => {
  const cyclic: Record<string, unknown> = {};
  cyclic.self = cyclic;
  const path = recordedFieldPath('root', cyclic);
  assert.equal(path.value, cyclic);
  assert.deepEqual(path.path, ['root', 'self']);
  let deep: unknown = 'leaf';
  for (let index = 0; index < 5000; index++) deep = { nested: deep };
  const flattened = recordedFieldPath('root', deep);
  assert.equal(flattened.path.length, 32);
  assert.equal(typeof flattened.value, 'object');
});

test('long text pages preserve emoji at boundaries and reproduce the exact source', () => {
  const source =
    'a'.repeat(RECORDED_TEXT_PAGE_SIZE - 1) +
    '😀' +
    'b'.repeat(RECORDED_TEXT_PAGE_SIZE - 2) +
    '🧑‍💻\nTail';
  let restored = '';
  for (let page = 0; page < Math.ceil(source.length / RECORDED_TEXT_PAGE_SIZE); page++) {
    const { start, end } = recordedTextRange(source, page);
    const part = source.slice(start, end);
    assert.equal(/^[\uDC00-\uDFFF]/.test(part), false);
    assert.equal(/[\uD800-\uDBFF]$/.test(part), false);
    restored += part;
  }
  assert.equal(restored, source);
});

test('recorded renderer preserves nested structure while bounding initial DOM work', async (t) => {
  const server = await createServer({
    root: fileURLToPath(new URL('..', import.meta.url)),
    configFile: false,
    server: { middlewareMode: true, watch: null, hmr: false, ws: false },
    appType: 'custom',
    optimizeDeps: { noDiscovery: true },
  });
  t.after(() => server.close());
  const { RecordedValue } = await server.ssrLoadModule('/src/RecordedValue.tsx');
  const render = (value: unknown) => renderToStaticMarkup(createElement(RecordedValue, { value }));

  await t.test('single-key chains show one semantic path with no disclosure clicks', () => {
    const html = render({ result: { file: { path: 'plans/conference.md' } } });
    assert.match(html, /Result/);
    assert.match(html, /File/);
    assert.match(html, /Path/);
    assert.match(html, /plans\/conference\.md/);
    assert.equal((html.match(/<dt /g) ?? []).length, 1);
    assert.equal(html.includes('<details'), false);
  });

  await t.test(
    'singleton objects inside array items expose their path without another fold',
    () => {
      const html = render({ artifacts: [{ result: { file: { path: 'plan.md' } } }] });
      assert.match(html, /plan\.md/);
      assert.match(html, /Result/);
      assert.match(html, /File/);
      assert.match(html, /Path/);
      assert.equal(html.includes('<details'), false);
      assert.match(html, /<ol class="recorded-array" start="1">/);
    }
  );

  await t.test('sibling groups retain all fields and arrays retain value order', () => {
    const html = render({
      artifact: { path: 'plan.md', checksum: 'abc' },
      approved: false,
      score: 0,
      notes: '',
      missing: null,
      labels: ['third', 'first', 'second'],
    });
    assert.match(html, /Artifact/);
    assert.match(html, /Checksum/);
    assert.match(html, /abc/);
    assert.match(html, />false</);
    assert.match(html, />0</);
    assert.match(html, />null</);
    assert.match(html, /&quot;&quot;/);
    assert.ok(html.indexOf('third') < html.indexOf('first'));
    assert.ok(html.indexOf('first') < html.indexOf('second'));
    assert.match(html, /<ol class="recorded-array" start="1">/);
  });

  await t.test('empty containers and scalar values are distinct and visible', () => {
    assert.match(render({}), />\{\}</);
    assert.match(render([]), />\[\]</);
    assert.match(render(''), /&quot;&quot;/);
    assert.match(render(false), />false</);
    assert.match(render(0), />0</);
    assert.match(render(-0), />-0</);
    assert.match(render(null), />null</);
    assert.match(render(undefined), /Not recorded/);
  });

  await t.test('HTML-like keys and values remain literal escaped text', () => {
    const html = render({ '<img src=x onerror=alert(1)>': '<script>alert(1)</script>\nText' });
    assert.equal(html.includes('<img'), false);
    assert.equal(html.includes('<script'), false);
    assert.match(html, /&lt;script&gt;/);
    assert.match(html, /&lt;img/);
    assert.match(html, /\nText/);
  });

  await t.test('large arrays and prose render a bounded page with explicit navigation', () => {
    const html = render(Array.from({ length: 10000 }, (_, index) => `entry-${index}-end`));
    assert.equal((html.match(/<li>/g) ?? []).length, 10);
    assert.match(html, /1–10 of 10000 items/);
    assert.match(html, /aria-label="Next items"/);
    assert.equal(html.includes('entry-10-end'), false);
    const long = render('x'.repeat(9000) + 'Tail');
    assert.match(long, /1–4000 of 9004 characters/);
    assert.equal(long.includes('Tail'), false);
  });

  await t.test('deep sibling groups are lazy and cyclic objects stop safely', () => {
    const html = render({
      report: {
        sources: { first: { note: 'Deep source' }, second: { note: 'Another source' } },
        status: 'checked',
      },
    });
    assert.match(html, /<details class="recorded-group"/);
    assert.match(html, /2 fields/);
    assert.equal(html.includes('Deep source'), false);
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    assert.match(render(cyclic), /Circular reference/);
  });

  await t.test('data inspection never invokes accessor code or renders object internals', () => {
    const source = Object.defineProperty({}, 'secret', {
      enumerable: true,
      get() {
        assert.fail('Getter must not execute');
      },
    });
    assert.match(render(source), /Unavailable property/);
    assert.match(render(new Date()), /Unsupported value/);
  });
});
