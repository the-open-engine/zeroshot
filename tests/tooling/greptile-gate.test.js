'use strict';

const assert = require('node:assert/strict');
const { it } = require('node:test');
const { runWorkflowStep } = require('./workflow-http-fixture');

const head = 'a'.repeat(40);
const endpoint = `/repos/fixture/repo/commits/${head}/check-runs?per_page=100`;
// The workflow runs on Ubuntu and requires its Bash, gh and jq executables.
const options = { skip: process.platform === 'win32' };

function check(id, status, app = 867647, name = 'Greptile Review') {
  return { id, status, app: { id: app }, name };
}

async function runGate(t, pages) {
  const result = await runWorkflowStep(t, {
    workflow: '.github/workflows/ci.yml',
    job: 'required',
    step: 'Wait for Greptile review',
    prefix: 'sleep() { echo "sleep:$1"; }\n',
    environment: { REVIEW_HEAD_SHA: head },
    respond(request, response, index) {
      const page = pages[index] ?? { status: 500 };
      if (page.next) {
        response.setHeader('Link', `<http://api.github.localhost${endpoint}&page=2>; rel="next"`);
      }
      response.writeHead(page.status ?? 200);
      response.end(
        JSON.stringify(
          page.status ? { message: 'fixture API failure' } : { check_runs: page.checks }
        )
      );
    },
  });
  return { ...result, requests: result.requests.map(({ url }) => url) };
}

it(
  'runs the real gh gate through missing, paginated pending and completed reviews',
  options,
  async (t) => {
    const ignored = [
      check(999, 'completed', 123),
      check(998, 'completed', 867647, 'Another check'),
    ];
    const result = await runGate(t, [
      { checks: ignored, next: true },
      { checks: [] },
      { checks: [check(10, 'completed')], next: true },
      { checks: [...ignored, check(11, 'in_progress')] },
      { checks: [check(10, 'completed')], next: true },
      { checks: [...ignored, check(11, 'completed')] },
    ]);
    assert.equal(result.code, 0, result.stderr);
    assert.deepEqual(
      result.requests,
      Array.from({ length: 3 }, () => [
        `http://api.github.localhost${endpoint}`,
        `http://api.github.localhost${endpoint}&page=2`,
      ]).flat()
    );
    assert.match(result.stdout, /\(pending\)/);
    assert.match(result.stdout, /\(in_progress\)/);
    assert.equal((result.stdout.match(/sleep:15/g) ?? []).length, 2);
  }
);

it('fails the real gh gate on API errors instead of waiting or succeeding', options, async (t) => {
  const result = await runGate(t, [{ status: 403 }]);
  assert.notEqual(result.code, 0);
  assert.equal(result.requests.length, 1);
  assert.match(result.stderr, /HTTP 403/);
  assert.doesNotMatch(result.stdout, /Waiting|sleep:/);
});
