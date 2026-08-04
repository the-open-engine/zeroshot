'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const { URL } = require('node:url');

const { resolveHostedRuntime } = require('./runtime-config');

const RUN_INTENT_VERSION = 'zeroshot.run-intent/v1';
const MAX_RUN_INTENT_BYTES = 10 * 1024 * 1024 + 64 * 1024;
const MAX_CREDENTIAL_BYTES = 4 * 1024 * 1024;
const CAPSULE_SIZES = new Set(['tiny', 'small', 'standard', 'large']);

function validRepository(value) {
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(value)) return false;
  return value.split('/').every((segment) => segment !== '.' && segment !== '..');
}

function repositoryFromRemote(cwd = process.cwd()) {
  let remote;
  try {
    remote = execFileSync('git', ['remote', 'get-url', 'origin'], {
      cwd,
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    }).trim();
  } catch {
    return null;
  }
  const match = remote.match(
    /^(?:git@github\.com:|ssh:\/\/git@github\.com\/|https?:\/\/github\.com\/)([^/]+\/[^/]+?)(?:\.git)?$/
  );
  return match && validRepository(match[1]) ? match[1] : null;
}

function isolationProfile(options = {}) {
  return options.pr ? 'isolation.pr@1' : 'isolation.worktree@1';
}

function providerProfile(options = {}) {
  return options.pr ? 'provider.hosted-pr@1' : 'provider.hosted@1';
}

function issueRequest(issue, options = {}) {
  return {
    source: 'issue',
    issue,
    artifacts: [],
    isolationProfile: isolationProfile(options),
    providerProfile: providerProfile(options),
  };
}

function promptRequest(prompt, options = {}) {
  return {
    source: 'prompt',
    prompt,
    artifacts: [],
    isolationProfile: isolationProfile(options),
    providerProfile: providerProfile(options),
  };
}

function issueInput(value, options = {}) {
  const shorthand = value.match(/^([A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+)#([1-9][0-9]*)$/);
  if (shorthand && validRepository(shorthand[1])) {
    const canonicalIssue = `https://github.com/${shorthand[1]}/issues/${shorthand[2]}`;
    return { repository: shorthand[1], request: issueRequest(canonicalIssue, options) };
  }
  let url;
  try {
    url = new URL(value);
  } catch {
    return null;
  }
  const match = url.pathname.match(/^\/([^/]+)\/([^/]+)\/issues\/([1-9][0-9]*)\/?$/);
  const repository = match ? `${match[1]}/${match[2]}` : '';
  if (url.hostname !== 'github.com' || !match || !validRepository(repository)) return null;
  return { repository, request: issueRequest(value, options) };
}

async function stdinText() {
  if (process.stdin.isTTY) throw new Error('zeroshot run - requires piped input');
  const chunks = [];
  let bytes = 0;
  for await (const chunk of process.stdin) {
    bytes += chunk.length;
    if (bytes > 1024 * 1024) throw new Error('hosted task input exceeds 1 MiB');
    chunks.push(chunk);
  }
  const value = Buffer.concat(chunks).toString('utf8').trim();
  if (!value) throw new Error('hosted task input is empty');
  return value;
}

function readTaskFile(filename) {
  let descriptor;
  try {
    descriptor = fs.openSync(filename, fs.constants.O_RDONLY | (fs.constants.O_NONBLOCK || 0));
  } catch (error) {
    if (['ENOENT', 'ENOTDIR', 'EISDIR'].includes(error.code)) return null;
    throw error;
  }
  try {
    if (!fs.fstatSync(descriptor).isFile()) return null;
    return fs.readFileSync(descriptor, 'utf8');
  } finally {
    fs.closeSync(descriptor);
  }
}

async function resolveInput(input, options) {
  const explicitIssue = issueInput(input, options);
  if (explicitIssue) return explicitIssue;
  const repository =
    options.repository || process.env.ZEROSHOT_REPOSITORY || repositoryFromRemote();
  if (!validRepository(repository || '')) {
    throw new Error(
      'hosted runs need a GitHub repository; use org/repo#123, --repository owner/name, ' +
        'ZEROSHOT_REPOSITORY, or run inside a GitHub checkout'
    );
  }
  if (/^[1-9][0-9]*$/.test(input)) return { repository, request: issueRequest(input, options) };
  if (input === '-') return { repository, request: promptRequest(await stdinText(), options) };
  const filename = path.resolve(input);
  const fileInput = readTaskFile(filename);
  if (fileInput !== null) {
    const text = fileInput.trim();
    if (!text) throw new Error(`hosted task file is empty: ${input}`);
    return { repository, request: promptRequest(text, options) };
  }
  if (!input.trim()) throw new Error('hosted task input is empty');
  return { repository, request: promptRequest(input.trim(), options) };
}

function githubToken(environment = process.env) {
  const configured = environment.GH_TOKEN || environment.GITHUB_TOKEN;
  if (configured?.trim()) return configured.trim();
  try {
    const token = execFileSync('gh', ['auth', 'token'], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    }).trim();
    if (token) return token;
  } catch {
    // The actionable error below covers missing and unauthenticated gh alike.
  }
  throw new Error('hosted runs require GH_TOKEN/GITHUB_TOKEN or an authenticated gh CLI');
}

function credentialsForRun(resolved, runtime, options, environment = process.env) {
  const credentials = {
    githubToken: githubToken(environment),
    repository: resolved.repository,
    runtime: resolveHostedRuntime(runtime, options, environment),
  };
  if (Buffer.byteLength(JSON.stringify(credentials)) > MAX_CREDENTIAL_BYTES) {
    throw new Error('hosted credential bundle exceeds 4 MiB');
  }
  return credentials;
}

async function buildHostedRun(input, runtime, options, environment = process.env) {
  const resolved = await resolveInput(input, options);
  return {
    credentials: credentialsForRun(resolved, runtime, options, environment),
    request: resolved.request,
  };
}

function validateHostedOptions(options) {
  const unsupported = [
    ['config', '--config'],
    ['docker', '--docker'],
    ['worktree', '--worktree'],
    ['dockerImage', '--docker-image'],
    ['strictSchema', '--strict-schema'],
    ['ship', '--ship'],
    ['prBase', '--pr-base'],
    ['mergeQueue', '--merge-queue'],
    ['closeIssue', '--close-issue'],
    ['workers', '--workers'],
    ['gitlab', '--gitlab'],
    ['jira', '--jira'],
    ['devops', '--devops'],
    ['linear', '--linear'],
    ['detach', '--detach'],
    ['mount', '--mount'],
    ['containerHome', '--container-home'],
  ];
  const selected = unsupported
    .filter(([name]) => options[name] !== undefined && options[name] !== false)
    .map(([, flag]) => flag);
  if (selected.length) {
    throw new Error(`hosted runs do not support ${selected.join(', ')}`);
  }
  if (!CAPSULE_SIZES.has(options.size || 'standard')) {
    throw new Error('hosted runs require --size tiny, small, standard, or large');
  }
}

function runIntentEnvelope(credentials, request) {
  return {
    version: RUN_INTENT_VERSION,
    credentials,
    request,
  };
}

module.exports = {
  MAX_RUN_INTENT_BYTES,
  RUN_INTENT_VERSION,
  buildHostedRun,
  credentialsForRun,
  issueInput,
  repositoryFromRemote,
  resolveInput,
  runIntentEnvelope,
  validateHostedOptions,
};
