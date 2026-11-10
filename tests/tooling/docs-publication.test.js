'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { it } = require('node:test');
const yaml = require('js-yaml');
const { runWorkflowStep } = require('./workflow-http-fixture');

const commit = 'a'.repeat(40);
const manifest = { docsVersion: 'v15.0', productVersion: '15.0.0', sourceCommit: commit };
const options = { skip: process.platform === 'win32' };
const stepName = 'Verify live documentation manifest';

function runVerification(t, replies, current = false) {
  return runWorkflowStep(t, {
    workflow: '.github/workflows/docs.yml',
    job: 'publish',
    step: stepName,
    prefix: 'export http_proxy="$HTTP_PROXY"\nsleep() { echo "sleep:$1"; }\n',
    environment: {
      DOCS_SITE_URL: 'http://pages.localhost/zeroshot/',
      DOCS_VERSION: current ? 'current' : 'v15.0',
      ZEROSHOT_PRODUCT_DOCS_VERSION: current ? '' : '15.0.0',
      SOURCE_COMMIT: commit,
    },
    respond(_request, response, index) {
      const reply = replies[Math.min(index, replies.length - 1)];
      response.writeHead(reply.status ?? 200);
      response.end(typeof reply.body === 'string' ? reply.body : JSON.stringify(reply.body));
    },
  });
}

function assertExhausted(result) {
  assert.notEqual(result.code, 0);
  assert.equal(result.requests.length, 120);
  assert.equal((result.stdout.match(/sleep:5/g) ?? []).length, 119);
  assert.match(result.stdout, /did not match after 120 attempts/);
  assert.doesNotMatch(result.stdout, /Verified public documentation/);
}

it(
  'checks the exact released public manifest after the official deployment',
  options,
  async (t) => {
    const workflow = yaml.load(
      fs.readFileSync(path.resolve(__dirname, '../../.github/workflows/docs.yml'), 'utf8')
    );
    const steps = workflow.jobs.publish.steps;
    const verify = steps.find((step) => step.name === stepName);
    assert.ok(steps.indexOf(verify) > steps.findIndex((step) => step.id === 'deployment'));
    assert.equal(verify.env.DOCS_SITE_URL, '${{ steps.deployment.outputs.page_url }}');
    assert.notEqual(verify['continue-on-error'], true);
    const result = await runVerification(t, [
      { body: { ...manifest, publisherCommit: 'b'.repeat(40) } },
    ]);
    assert.equal(result.code, 0, result.stderr);
    assert.deepEqual(result.requests, [
      {
        method: 'GET',
        url: 'http://pages.localhost/zeroshot/v15.0/manifest.json',
        body: '',
      },
    ]);
    assert.match(result.stdout, /Verified public documentation manifest/);
    assert.doesNotMatch(result.stdout, /sleep:/);
  }
);

it('requires an explicit null product version for Current', options, async (t) => {
  const current = { ...manifest, docsVersion: 'current', productVersion: null };
  const success = await runVerification(t, [{ body: current }], true);
  assert.equal(success.code, 0, success.stderr);
  assert.equal(success.requests[0].url, 'http://pages.localhost/zeroshot/current/manifest.json');
  delete current.productVersion;
  assertExhausted(await runVerification(t, [{ body: current }], true));
});

it(
  'retries unavailable, stale and malformed responses until the exact manifest is live',
  options,
  async (t) => {
    const result = await runVerification(t, [
      { status: 404, body: {} },
      { status: 503, body: {} },
      { body: { ...manifest, sourceCommit: 'b'.repeat(40) } },
      { body: '<html>deployment pending</html>' },
      { body: manifest },
    ]);
    assert.equal(result.code, 0, result.stderr);
    assert.equal(result.requests.length, 5);
    assert.ok(
      result.requests.every(
        ({ method, url }) =>
          method === 'GET' && url === 'http://pages.localhost/zeroshot/v15.0/manifest.json'
      )
    );
    assert.equal((result.stdout.match(/sleep:5/g) ?? []).length, 4);
  }
);

it(
  'fails bounded polling for wrong identities and malformed HTTP 200 manifests',
  options,
  async (t) => {
    for (const body of [
      { ...manifest, docsVersion: 'v14.0' },
      { ...manifest, productVersion: '14.0.0' },
      { ...manifest, sourceCommit: 'b'.repeat(40) },
      [],
      'not json',
    ]) {
      assertExhausted(await runVerification(t, [{ body }]));
    }
  }
);

it('fails bounded polling when public documentation stays unavailable', options, async (t) => {
  for (const status of [404, 503]) {
    assertExhausted(await runVerification(t, [{ status, body: {} }]));
  }
});
