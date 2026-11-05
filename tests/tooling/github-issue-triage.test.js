'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { it } = require('node:test');
const yaml = require('js-yaml');
const { reconcileIssueLabels, githubRequest } = require('../../scripts/github-issue-triage');

const maintainer = { id: 1, login: 'maintainer', type: 'User' };
const writer = { id: 2, login: 'writer', type: 'User' };
const service = { id: 3, login: 'zeroshot-app[bot]', type: 'Bot' };
const guard = { id: 4, login: 'github-actions[bot]', type: 'Bot' };
const origin = '/repos/the-open-engine/zeroshot';
const timestamp = (index) => new Date(1700000000000 + index * 1000).toISOString();

function event(kind, name, actor, index) {
  return { event: kind, label: { name }, actor, created_at: timestamp(index) };
}

function readFixture({ events, labels, options }, route) {
  if (route.includes('/events?')) {
    const page = Number(new URL('https://api.github.com' + route).searchParams.get('page'));
    return events.slice((page - 1) * 100, page * 100);
  }
  if (route.includes('/collaborators/')) {
    if (options.permissionError) throw options.permissionError;
    const login = route.split('/collaborators/')[1].split('/')[0];
    return options.roles?.[login] ?? { permission: 'write', role_name: 'write' };
  }
  if (route === origin + '/issues/7') {
    return { labels: [...labels].map((name) => ({ name })) };
  }
  throw new Error('Unexpected fixture read: ' + route);
}

function fixture(options = {}) {
  const events = options.events ?? [];
  const trigger = options.trigger ?? events.at(-1);
  const payload = {
    action: options.action ?? 'opened',
    repository: { full_name: 'the-open-engine/zeroshot' },
    issue: { number: 7, updated_at: trigger?.created_at ?? timestamp(0) },
    sender: trigger?.actor ?? writer,
    label: trigger?.label,
  };
  const labels = new Set(options.labels ?? []);
  const calls = [];
  const request = async (method, route, body) => {
    calls.push({ method, route, body });
    if (method === 'GET') return readFixture({ events, labels, options }, route);
    if (method === 'POST' && route.endsWith('/labels')) {
      for (const label of body.labels) labels.add(label);
      return [];
    }
    if (method === 'DELETE') {
      labels.delete(decodeURIComponent(route.split('/labels/')[1]));
      return [];
    }
    throw new Error('Unexpected fixture request: ' + method + ' ' + route);
  };
  return {
    payload,
    labels,
    calls,
    run: () =>
      reconcileIssueLabels({
        repository: 'the-open-engine/zeroshot',
        payload,
        request,
      }),
  };
}

const managedRoles = { maintainer: { permission: 'write', role_name: 'maintain' } };
const names = (f) => [...f.labels].sort();

it('new issues get triage and cannot retain labels assigned by an ordinary writer', async () => {
  const f = fixture({
    labels: ['auto-fix-0', 'bug'],
    events: [event('labeled', 'auto-fix-0', writer, 1), event('labeled', 'bug', writer, 2)],
  });
  await f.run();
  assert.deepEqual(names(f), ['triage']);
  assert.equal(f.calls.filter((call) => call.route.includes('/collaborators/')).length, 1);
});

it('Maintain role is accepted even when REST maps its permission to write', async () => {
  const f = fixture({
    labels: ['bug', 'plan-1'],
    roles: managedRoles,
    events: [
      event('labeled', 'bug', maintainer, 1),
      event('labeled', 'plan-1', maintainer, 2),
      event('unlabeled', 'triage', maintainer, 3),
    ],
  });
  await f.run();
  assert.deepEqual(names(f), ['bug', 'plan-1']);
  assert.equal(f.calls.filter((call) => call.method !== 'GET').length, 0);
});

it('Write and Triage roles cannot authorize labels', async () => {
  for (const role_name of ['write', 'triage', 'read']) {
    const f = fixture({
      roles: { writer: { permission: 'write', role_name } },
      labels: ['plan-0'],
      events: [event('labeled', 'plan-0', writer, 1)],
    });
    await f.run();
    assert.deepEqual(names(f), ['triage']);
  }
});

it('the verified service App may assign routing labels and clear triage', async () => {
  const f = fixture({
    events: [
      event('labeled', 'triage', guard, 1),
      event('labeled', 'auto-fix-2', service, 2),
      event('unlabeled', 'triage', service, 3),
    ],
    labels: ['auto-fix-2'],
  });
  await f.run();
  assert.deepEqual(names(f), ['auto-fix-2']);
  assert.equal(f.calls.filter((call) => call.route.includes('/collaborators/')).length, 0);
});

it('a matching login without the verified Bot type grants no service authority', async () => {
  const impersonator = { ...service, type: 'User' };
  const f = fixture({
    events: [event('labeled', 'auto-fix-0', impersonator, 1)],
    labels: ['auto-fix-0'],
  });
  await f.run();
  assert.deepEqual(names(f), ['triage']);
});

it('unauthorized removal restores the last maintainer label decision', async () => {
  const removed = event('unlabeled', 'plan-0', writer, 2);
  const f = fixture({
    action: 'unlabeled',
    trigger: removed,
    roles: managedRoles,
    events: [event('labeled', 'plan-0', maintainer, 1), removed],
    labels: ['triage'],
  });
  await f.run();
  assert.deepEqual(names(f), ['plan-0', 'triage']);
});

it('a stale webhook cannot undo a later maintainer decision', async () => {
  const untrusted = event('labeled', 'auto-fix-0', writer, 1);
  const f = fixture({
    action: 'labeled',
    trigger: untrusted,
    roles: managedRoles,
    events: [
      untrusted,
      event('unlabeled', 'auto-fix-0', maintainer, 2),
      event('labeled', 'plan-1', maintainer, 3),
      event('unlabeled', 'triage', maintainer, 4),
    ],
    labels: ['plan-1'],
  });
  await f.run();
  assert.deepEqual(names(f), ['plan-1']);
  assert.equal(f.calls.filter((call) => call.method !== 'GET').length, 0);
});

it('guard corrections never supersede later maintainer intent during a race', async () => {
  const f = fixture({
    roles: managedRoles,
    events: [
      event('labeled', 'plan-1', writer, 1),
      event('labeled', 'plan-1', maintainer, 2),
      event('unlabeled', 'plan-1', guard, 3),
    ],
    labels: ['triage'],
  });
  await f.run();
  assert.deepEqual(names(f), ['plan-1', 'triage']);
  const second = fixture({
    roles: managedRoles,
    events: [
      event('labeled', 'plan-1', maintainer, 2),
      event('unlabeled', 'plan-1', guard, 3),
      event('labeled', 'plan-1', guard, 4),
    ],
    labels: ['plan-1', 'triage'],
  });
  await second.run();
  assert.equal(second.calls.filter((call) => call.method !== 'GET').length, 0);
});

it('one coalesced invocation reconciles unauthorized labels from earlier events too', async () => {
  const latest = event('labeled', 'plan-3', writer, 2);
  const f = fixture({
    action: 'labeled',
    trigger: latest,
    events: [event('labeled', 'auto-fix-0', writer, 1), latest],
    labels: ['triage', 'auto-fix-0', 'plan-3'],
  });
  await f.run();
  assert.deepEqual(names(f), ['triage']);
});

it('all event pages are read before restoring an approved label', async () => {
  const events = Array.from({ length: 100 }, (_, index) => ({ event: 'commented', id: index }));
  events.push(event('labeled', 'plan-2', maintainer, 101));
  const f = fixture({ events, roles: managedRoles, labels: ['triage'] });
  await f.run();
  assert.deepEqual(names(f), ['plan-2', 'triage']);
  assert.equal(f.calls.filter((call) => call.route.includes('/events?')).length, 2);
});

it('a reopened issue returns to triage unless a later approved decision already cleared it', async () => {
  const events = [
    event('unlabeled', 'triage', service, 1),
    event('closed', undefined, maintainer, 2),
    event('reopened', undefined, writer, 3),
  ];
  const f = fixture({ action: 'reopened', events });
  await f.run();
  assert.deepEqual(names(f), ['triage']);
  const stale = fixture({
    action: 'reopened',
    events: [
      ...events,
      event('labeled', 'plan-1', service, 4),
      event('unlabeled', 'triage', service, 5),
    ],
    labels: ['plan-1'],
  });
  await stale.run();
  assert.deepEqual(names(stale), ['plan-1']);
});

it('unavailable role evidence fails before changing any labels', async () => {
  const f = fixture({
    events: [event('labeled', 'auto-fix-0', writer, 1)],
    labels: ['auto-fix-0'],
    permissionError: new Error('metadata unavailable'),
  });
  await assert.rejects(f.run(), /metadata unavailable/);
  assert.equal(f.calls.filter((call) => call.method !== 'GET').length, 0);
});

it('an event missing from the API history cannot trigger destructive reconciliation', async () => {
  const f = fixture({
    action: 'labeled',
    trigger: event('labeled', 'plan-0', maintainer, 10),
    roles: managedRoles,
    events: [event('unlabeled', 'plan-0', maintainer, 9)],
    labels: ['plan-0'],
  });
  await assert.rejects(f.run(), /not yet visible/);
  assert.equal(f.calls.filter((call) => call.method !== 'GET').length, 0);
});

it('PR payloads and mismatched repository events cannot write issue labels', async () => {
  const f = fixture();
  f.payload.issue.pull_request = {};
  assert.deepEqual(await f.run(), { skipped: 'pull_request' });
  assert.equal(f.calls.length, 0);
  const invalid = fixture();
  invalid.payload.repository.full_name = 'attacker/other';
  await assert.rejects(invalid.run(), /Invalid issue triage event/);
  assert.equal(invalid.calls.length, 0);
});

it('the API client pins GitHub authority, rejects redirects and never reports response bodies', async () => {
  const previous = global.fetch;
  const calls = [];
  global.fetch = async (url, options) => {
    calls.push({ url, options });
    return { ok: false, status: 403, json: () => assert.fail('private response was read') };
  };
  try {
    await assert.rejects(
      githubRequest('test-token')('GET', origin + '/issues/7'),
      /^Error: GitHub issue triage request failed \(403\)$/
    );
    assert.equal(calls[0].url, 'https://api.github.com' + origin + '/issues/7');
    assert.equal(calls[0].options.redirect, 'error');
    assert.equal(calls[0].options.headers.authorization, 'Bearer test-token');
  } finally {
    global.fetch = previous;
  }
});

it('workflow uses trusted default-branch code and the minimum issue-write permissions', () => {
  const source = fs.readFileSync(
    path.join(__dirname, '../../.github/workflows/issue-triage.yml'),
    'utf8'
  );
  const workflow = yaml.load(source);
  assert.deepEqual(workflow.on.issues.types, ['opened', 'reopened', 'labeled', 'unlabeled']);
  assert.deepEqual(workflow.permissions, { contents: 'read', issues: 'write' });
  assert.equal(workflow.concurrency['cancel-in-progress'], false);
  const steps = workflow.jobs.labels.steps;
  const checkout = steps.find((step) => step.uses?.startsWith('actions/checkout@'));
  assert.equal(checkout.with.ref, '${{ github.event.repository.default_branch }}');
  assert.equal(checkout.with['persist-credentials'], false);
  assert.ok(steps.filter((step) => step.uses).every((step) => /@[a-f0-9]{40}/.test(step.uses)));
  assert.equal(steps.at(-1).run, 'node scripts/github-issue-triage.js');
  assert.equal(source.includes('github.event.issue.body'), false);
});
