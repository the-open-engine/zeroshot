'use strict';

const assert = require('node:assert/strict');
const { it } = require('node:test');
const { runWorkflowStep } = require('./workflow-http-fixture');

const commit = 'a'.repeat(40);
const runPath = '/repos/fixture/repo/actions/runs/42';
const options = { skip: process.platform === 'win32' };

function runDispatch(t, fixture = {}) {
  let runReads = 0;
  return runWorkflowStep(t, {
    prefix: 'sleep() { :; }\n',
    workflow: '.github/workflows/release.yml',
    job: 'python-sdk-release',
    step: 'Dispatch and await standalone Python SDK release',
    environment: {
      RELEASE_VERSION: '10.19.0',
      RELEASE_COMMIT: commit,
      PUBLISH_PYPI: fixture.publishPyPi ?? 'true',
    },
    respond(request, response) {
      if (request.method === 'POST') {
        response.writeHead(fixture.dispatchStatus ?? 200);
        response.end(JSON.stringify(fixture.receipt ?? { workflow_run_id: 42 }));
      } else if (new URL(request.url).pathname.endsWith('/workflows/37')) {
        response.writeHead(200);
        response.end(JSON.stringify({ id: 37, name: 'Release Python SDK' }));
      } else {
        runReads += 1;
        if (runReads <= (fixture.notFoundReads ?? 0)) {
          response.writeHead(404);
          response.end(JSON.stringify({ message: 'Not Found' }));
          return;
        }
        response.writeHead(200);
        response.end(
          JSON.stringify({
            id: 42,
            workflow_id: 37,
            name: 'Release Python SDK',
            status: 'completed',
            conclusion: fixture.conclusion ?? 'success',
            event: 'workflow_dispatch',
          })
        );
      }
    },
  });
}

it(
  'dispatches exact SDK inputs and waits for the returned run with the real gh CLI',
  options,
  async (t) => {
    for (const publishPyPi of ['true', 'false']) {
      const result = await runDispatch(t, { publishPyPi });
      assert.equal(result.code, 0, result.stderr);
      assert.deepEqual(result.requests[0], {
        method: 'POST',
        url: 'http://api.github.localhost/repos/fixture/repo/actions/workflows/release-python.yml/dispatches',
        body: {
          ref: 'main',
          return_run_details: true,
          inputs: {
            action: 'release',
            zeroshot_version: '10.19.0',
            sdk_revision: '1',
            release_commit: commit,
            publish_pypi: publishPyPi,
          },
        },
      });
      assert.ok(result.requests.slice(1).length > 0, 'must await the dispatched run');
      assert.ok(
        result.requests
          .slice(1)
          .every(({ url }) =>
            [runPath, '/repos/fixture/repo/actions/workflows/37'].includes(new URL(url).pathname)
          ),
        JSON.stringify(result.requests)
      );
      assert.match(result.stdout, /https:\/\/github.localhost\/fixture\/repo\/actions\/runs\/42/);
    }
  }
);

it(
  'waits for the exact dispatched run to become readable after transient 404s',
  options,
  async (t) => {
    const result = await runDispatch(t, { notFoundReads: 2 });
    assert.equal(result.code, 0, result.stderr);
    assert.equal(result.requests.filter(({ method }) => method === 'POST').length, 1);
    const reads = result.requests.filter(({ url }) => new URL(url).pathname === runPath);
    assert.ok(reads.length >= 4, 'must retry readiness before watching the same run');
    assert.ok(
      result.requests.every(
        ({ method, url }) =>
          method === 'POST' ||
          [runPath, '/repos/fixture/repo/actions/workflows/37'].includes(new URL(url).pathname)
      )
    );
  }
);

it('bounds readiness retries without redispatching or watching another run', options, async (t) => {
  const result = await runDispatch(t, { notFoundReads: Infinity });
  assert.notEqual(result.code, 0);
  assert.match(result.stdout, /did not become readable after 12 attempts/);
  assert.equal(result.requests.filter(({ method }) => method === 'POST').length, 1);
  const reads = result.requests.filter(({ method }) => method === 'GET');
  assert.equal(reads.length, 12);
  assert.ok(reads.every(({ url }) => new URL(url).pathname === runPath));
});

it(
  'propagates failed or cancelled SDK runs instead of permitting documentation',
  options,
  async (t) => {
    for (const conclusion of ['failure', 'cancelled', 'timed_out']) {
      const result = await runDispatch(t, { conclusion, notFoundReads: 2 });
      assert.notEqual(result.code, 0, conclusion);
      assert.ok(result.requests.some(({ method }) => method === 'GET'));
    }
  }
);

it(
  'fails on dispatch errors or a missing run receipt without watching unrelated runs',
  options,
  async (t) => {
    for (const fixture of [
      { dispatchStatus: 403 },
      { receipt: {} },
      { receipt: { workflow_run_id: 0 } },
    ]) {
      const result = await runDispatch(t, fixture);
      assert.notEqual(result.code, 0);
      assert.equal(result.requests.length, 1);
    }
  }
);
