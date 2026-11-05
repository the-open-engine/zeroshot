'use strict';

const fs = require('node:fs');

const SERVICE_BOT = 'zeroshot-app[bot]';
const ACTIONS_BOT = 'github-actions[bot]';

function labelName(label) {
  if (typeof label?.name !== 'string' || !label.name) throw new Error('Invalid issue label');
  return label.name;
}

function labelsOf(issue) {
  if (!Array.isArray(issue.labels)) throw new Error('Invalid issue label list');
  return new Set(issue.labels.map(labelName));
}

async function mayManage(actor, repository, serviceBot, request, permissions) {
  if (!actor || typeof actor.login !== 'string') return false;
  if (actor.login === ACTIONS_BOT) return false;
  if (actor.type === 'Bot' && actor.login === serviceBot) return true;
  if (actor.type !== 'User') return false;
  if (!permissions.has(actor.login)) {
    const result = await request(
      'GET',
      '/repos/' + repository + '/collaborators/' + encodeURIComponent(actor.login) + '/permission'
    );
    const role = result.role_name ?? result.permission;
    permissions.set(actor.login, role === 'maintain' || role === 'admin');
  }
  return permissions.get(actor.login);
}

async function labelHistory(repository, number, request) {
  const events = [];
  for (let page = 1; ; page++) {
    const values = await request(
      'GET',
      '/repos/' + repository + '/issues/' + number + '/events?per_page=100&page=' + page
    );
    if (!Array.isArray(values)) throw new Error('Invalid issue event page');
    events.push(...values);
    if (values.length < 100) return events;
  }
}

function requireVisibleEvent(payload, events) {
  if (!['labeled', 'unlabeled'].includes(payload.action)) return;
  const name = labelName(payload.label);
  const since = Date.parse(payload.issue.updated_at);
  if (
    !Number.isFinite(since) ||
    !events.some(
      (event) =>
        event.event === payload.action &&
        event.label?.name === name &&
        event.actor?.id === payload.sender?.id &&
        Date.parse(event.created_at) >= since
    )
  ) {
    throw new Error('Triggering label event is not yet visible; retry this workflow');
  }
}

async function approvedLabels(events, repository, serviceBot, request) {
  const desired = new Set(['triage']);
  const permissions = new Map();
  for (const event of events) {
    if (event.event === 'closed') desired.delete('triage');
    if (event.event === 'reopened') desired.add('triage');
    if (!['labeled', 'unlabeled'].includes(event.event)) continue;
    const name = labelName(event.label);
    if (!(await mayManage(event.actor, repository, serviceBot, request, permissions))) continue;
    if (event.event === 'labeled') desired.add(name);
    else desired.delete(name);
  }
  return desired;
}

function requireIssueEvent(payload, repository) {
  const number = payload.issue?.number;
  if (
    !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository ?? '') ||
    payload.repository?.full_name !== repository ||
    !Number.isSafeInteger(number) ||
    number < 1 ||
    !['opened', 'reopened', 'labeled', 'unlabeled'].includes(payload.action)
  ) {
    throw new Error('Invalid issue triage event');
  }
  return number;
}

async function reconcileIssueLabels({ request, payload, repository, serviceBot = SERVICE_BOT }) {
  if (payload.issue?.pull_request) return { skipped: 'pull_request' };
  const number = requireIssueEvent(payload, repository);
  const events = await labelHistory(repository, number, request);
  requireVisibleEvent(payload, events);
  const desired = await approvedLabels(events, repository, serviceBot, request);
  const prefix = '/repos/' + repository + '/issues/' + number;
  const current = labelsOf(await request('GET', prefix));
  const added = [...desired].filter((name) => !current.has(name));
  const removed = [...current].filter((name) => !desired.has(name));
  if (added.length) await request('POST', prefix + '/labels', { labels: added });
  for (const name of removed) {
    try {
      await request('DELETE', prefix + '/labels/' + encodeURIComponent(name));
    } catch (error) {
      if (error.status !== 404) throw error;
    }
  }
  return { issue: number, added: added.length, removed: removed.length };
}

function githubRequest(token) {
  if (!token) throw new Error('GitHub issue token is required');
  return async (method, pathname, body) => {
    if (!pathname.startsWith('/repos/')) throw new Error('Invalid GitHub issue API path');
    const response = await fetch('https://api.github.com' + pathname, {
      method,
      redirect: 'error',
      signal: AbortSignal.timeout(30000),
      headers: {
        authorization: 'Bearer ' + token,
        accept: 'application/vnd.github+json',
        'x-github-api-version': '2022-11-28',
        ...(body ? { 'content-type': 'application/json' } : {}),
      },
      ...(body ? { body: JSON.stringify(body) } : {}),
    });
    if (!response.ok) {
      const error = new Error('GitHub issue triage request failed (' + response.status + ')');
      error.status = response.status;
      throw error;
    }
    return response.status === 204 ? null : response.json();
  };
}

async function main() {
  const payload = JSON.parse(fs.readFileSync(process.env.GITHUB_EVENT_PATH, 'utf8'));
  const result = await reconcileIssueLabels({
    request: githubRequest(process.env.GITHUB_TOKEN),
    payload,
    repository: process.env.GITHUB_REPOSITORY,
  });
  process.stdout.write(JSON.stringify(result) + '\n');
}

module.exports = { reconcileIssueLabels, githubRequest };

if (require.main === module) {
  main().catch((error) => {
    process.stderr.write(error.message + '\n');
    process.exitCode = 1;
  });
}
