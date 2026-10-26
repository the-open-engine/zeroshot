'use strict';

const assert = require('node:assert/strict');
const { it } = require('node:test');
const { runWorkflowStep } = require('./workflow-http-fixture');

const commit = 'a'.repeat(40);
const options = { skip: process.platform === 'win32' };

function runDispatch(t, fixture = {}) {
  return runWorkflowStep(t, {
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
            [
              '/repos/fixture/repo/actions/runs/42',
              '/repos/fixture/repo/actions/workflows/37',
            ].includes(new URL(url).pathname)
          ),
        JSON.stringify(result.requests)
      );
      assert.match(result.stdout, /https:\/\/github.localhost\/fixture\/repo\/actions\/runs\/42/);
    }
  }
);

it(
  'propagates failed or cancelled SDK runs instead of permitting documentation',
  options,
  async (t) => {
    for (const conclusion of ['failure', 'cancelled', 'timed_out']) {
      const result = await runDispatch(t, { conclusion });
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
