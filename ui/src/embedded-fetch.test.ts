import test from 'node:test';
import assert from 'node:assert/strict';
import { ApiError } from './api';
import { createEmbeddedFetch } from './embedded-fetch';
const base = new URL('https://cloud.example/_bff/org/workspace/');
const csrf = { cookieName: '__Host-zsc-csrf', headerName: 'X-Zeroshot-CSRF' };
test('CSRF comes from the current unique cookie only on service POST writes', async () => {
  const calls: RequestInit[] = [];
  const urls: string[] = [];
  let cookie = '__Host-zsc-csrf=first_token';
  const fetcher = createEmbeddedFetch(
    base,
    csrf,
    () => cookie,
    async (input, init) => {
      urls.push(String(input));
      calls.push(init ?? {});
      return Response.json({ valid: true });
    }
  );
  await fetcher(new URL('bootstrap', base));
  await fetcher(new URL('validate', base), { method: 'POST' });
  cookie = '__Host-zsc-csrf=second_token';
  await fetcher('data', { method: 'POST' });
  assert.equal(new Headers(calls[0].headers).get(csrf.headerName), null);
  assert.equal(new Headers(calls[1].headers).get(csrf.headerName), 'first_token');
  assert.equal(new Headers(calls[2].headers).get(csrf.headerName), 'second_token');
  assert.equal(urls[2], new URL('data', base).href);
  assert.ok(calls.every((call) => call.redirect === 'error' && call.credentials === 'same-origin'));
});
test('missing/duplicate cookies and foreign service origins never send a token', async () => {
  let calls = 0;
  for (const cookie of ['', '__Host-zsc-csrf=one; __Host-zsc-csrf=two']) {
    const fetcher = createEmbeddedFetch(
      base,
      csrf,
      () => cookie,
      async () => {
        calls++;
        return Response.json({});
      }
    );
    await assert.rejects(
      fetcher(new URL('validate', base), { method: 'POST' }),
      (error: unknown) => error instanceof ApiError && error.code === 'csrf_missing'
    );
  }
  const fetcher = createEmbeddedFetch(
    base,
    csrf,
    () => '__Host-zsc-csrf=secret',
    async () => {
      calls++;
      return Response.json({});
    }
  );
  await assert.rejects(fetcher('https://foreign.example/validate', { method: 'POST' }));
  await assert.rejects(fetcher('https://cloud.example/outside/validate', { method: 'POST' }));
  assert.equal(calls, 0);
});
