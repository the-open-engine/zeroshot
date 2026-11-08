'use strict';

const assert = require('node:assert/strict');
const { mkdtemp, readFile, rm, writeFile, access } = require('node:fs/promises');
const { tmpdir } = require('node:os');
const path = require('node:path');
const { promisify } = require('node:util');
const { execFile } = require('node:child_process');
const { test } = require('node:test');
const { downloadReleaseAsset } = require('../../scripts/distribution/github-release-asset');

const command = promisify(execFile);
const api = 'https://api.github.com/repos/example/tool/releases/';
const content = Buffer.from('verified release bytes');
const signed = 'https://release-assets.githubusercontent.com/asset?signed=private';

async function fixture(t, changes = {}) {
  const root = await mkdtemp(path.join(tmpdir(), 'release-asset-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const calls = [];
  const asset = { name: 'tool.tar.gz', url: `${api}assets/42`, size: content.length, ...changes };
  const request = {
    repository: 'example/tool',
    tag: 'v1.0.0',
    name: asset.name,
    output: path.join(root, 'tool.tar.gz'),
    token: 'fixture-credential',
    fetcher: async (url, options) => {
      calls.push({ url, ...options });
      if (url === `${api}tags/v1.0.0`) return Response.json({ assets: [asset] });
      if (url === asset.url)
        return new Response(null, { status: 302, headers: { location: signed } });
      assert.equal(url, signed);
      return new Response(content);
    },
  };
  return { root, request, calls };
}

test('GitHub credentials stay on the API and never follow the signed asset redirect', async (t) => {
  const f = await fixture(t);
  await downloadReleaseAsset(f.request);
  assert.deepEqual(await readFile(f.request.output), content);
  assert.deepEqual(
    f.calls.map(({ url }) => url),
    [`${api}tags/v1.0.0`, `${api}assets/42`, signed]
  );
  assert.equal(f.calls[0].headers.Authorization, 'Bearer fixture-credential');
  assert.equal(f.calls[1].headers.Authorization, 'Bearer fixture-credential');
  assert.equal(f.calls[2].headers, undefined);
  assert.equal(f.calls[1].redirect, 'manual');
  assert.equal(f.calls[2].redirect, 'error');
  await assert.rejects(downloadReleaseAsset(f.request), { code: 'EEXIST' });
  assert.deepEqual(await readFile(f.request.output), content);
});

test('public builds can fetch assets without an ambient credential', async (t) => {
  const f = await fixture(t);
  delete f.request.token;
  await downloadReleaseAsset(f.request);
  assert.ok(f.calls.every(({ headers }) => headers?.Authorization === undefined));
});

test('release metadata cannot redirect the credential to another API asset owner', async (t) => {
  const f = await fixture(t, { url: 'https://api.github.com/repos/other/tool/releases/assets/42' });
  await assert.rejects(downloadReleaseAsset(f.request), /missing or ambiguous/);
  assert.equal(f.calls.length, 1);
  await assert.rejects(access(f.request.output));
});

test('insecure storage redirects and truncated assets fail before publishing a file', async (t) => {
  for (const bad of ['insecure', 'size']) {
    const f = await fixture(t, bad === 'size' ? { size: content.length + 1 } : {});
    if (bad === 'insecure')
      f.request.fetcher = async (url) =>
        url.includes('/tags/')
          ? Response.json({
              assets: [{ name: 'tool.tar.gz', url: `${api}assets/42`, size: content.length }],
            })
          : new Response(null, {
              status: 302,
              headers: { location: 'http://storage.invalid/file' },
            });
    await assert.rejects(
      downloadReleaseAsset(f.request),
      bad === 'insecure' ? /not HTTPS/ : /unexpected size/
    );
    await assert.rejects(access(f.request.output));
  }
});

test('Restic source-only staging still refuses a prefetched archive that violates the immutable pin', async (t) => {
  const root = await mkdtemp(path.join(tmpdir(), 'restic-source-pin-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const archive = path.join(root, 'wrong.tar.gz'),
    output = path.join(root, 'output');
  await writeFile(archive, 'not the pinned Restic source');
  await assert.rejects(
    command(
      'bash',
      [
        path.resolve(__dirname, '../../scripts/distribution/build-restic.sh'),
        output,
        '--source-only',
      ],
      { env: { ...process.env, ZEROSHOT_RESTIC_SOURCE_ARCHIVE: archive } }
    ),
    (error) => {
      assert.equal(error.code, 1);
      assert.match(error.stderr, /checksum did NOT match|computed checksum did NOT match/);
      return true;
    }
  );
  await assert.rejects(access(path.join(output, 'restic.tar.gz')));
});
