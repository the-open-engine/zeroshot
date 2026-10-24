'use strict';

const assert = require('node:assert/strict');
const { execFile } = require('node:child_process');
const { mkdtemp, readFile, rm } = require('node:fs/promises');
const { createServer } = require('node:http');
const os = require('node:os');
const path = require('node:path');
const { it } = require('node:test');
const yaml = require('js-yaml');

const root = path.resolve(__dirname, '..', '..');
const head = 'a'.repeat(40);
const endpoint = `/repos/fixture/repo/commits/${head}/check-runs?per_page=100`;
// The workflow runs on Ubuntu and requires its Bash, gh and jq executables.
const options = { skip: process.platform === 'win32' };

function check(id, status, app = 867647, name = 'Greptile Review') {
  return { id, status, app: { id: app }, name };
}

async function runGate(t, pages) {
  const home = await mkdtemp(path.join(os.tmpdir(), 'zeroshot-greptile-'));
  t.after(() => rm(home, { recursive: true, force: true }));
  const requests = [];
  const server = createServer((request, response) => {
    requests.push(request.url);
    const page = pages[requests.length - 1] ?? { status: 500 };
    if (page.next) {
      response.setHeader('Link', `<http://api.github.localhost${endpoint}&page=2>; rel="next"`);
    }
    response.writeHead(page.status ?? 200, {
      'Content-Type': 'application/json',
      Connection: 'close',
    });
    response.end(
      JSON.stringify(page.status ? { message: 'fixture API failure' } : { check_runs: page.checks })
    );
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise((resolve) => server.close(resolve)));

  const workflow = yaml.load(await readFile(path.join(root, '.github/workflows/ci.yml'), 'utf8'));
  const step = workflow.jobs.required.steps.find(({ name }) => name === 'Wait for Greptile review');
  assert.ok(step?.run, 'required CI must contain the Greptile gate');
  const result = await new Promise((resolve, reject) => {
    execFile(
      'bash',
      ['--noprofile', '--norc', '-c', `sleep() { printf 'sleep:%s\\n' "$1"; }\n${step.run}`],
      {
        timeout: 10000,
        env: {
          PATH: process.env.PATH,
          HOME: home,
          GH_CONFIG_DIR: home,
          GH_HOST: 'github.localhost',
          GH_TOKEN: 'offline-test-token',
          GH_ENTERPRISE_TOKEN: 'offline-test-token',
          GH_PROMPT_DISABLED: '1',
          GH_NO_UPDATE_NOTIFIER: '1',
          // gh uses HTTP for *.localhost; only the local proxy receives requests.
          HTTP_PROXY: `http://127.0.0.1:${server.address().port}`,
          NO_PROXY: '',
          GITHUB_REPOSITORY: 'fixture/repo',
          REVIEW_HEAD_SHA: head,
        },
      },
      (error, stdout, stderr) => {
        if (error && !Number.isInteger(error.code)) return reject(error);
        resolve({ code: error?.code ?? 0, stdout, stderr });
      }
    );
  });
  return { ...result, requests };
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
