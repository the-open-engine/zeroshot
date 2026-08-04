'use strict';

const crypto = require('node:crypto');

const {
  MAX_RUN_INTENT_BYTES,
  buildHostedRun,
  runIntentEnvelope,
  validateHostedOptions,
} = require('./contract');
const { HostedHttpError, request } = require('./http');
const { createHostedTargetSession, organizationFromToken } = require('./target-session');

const RUN_INTENT_POLL_MS = 500;
const TERMINAL_STATES = new Set(['succeeded', 'failed', 'cancelled', 'expired']);
const RUN_INTENT_STATES = new Set([
  'queued',
  'provisioning',
  'running',
  'cancelling',
  ...TERMINAL_STATES,
]);
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const SUBMISSION_KEY = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function validateOptions(options) {
  validateHostedOptions({ ...options, detach: undefined });
  if (options.submissionKey !== undefined && !SUBMISSION_KEY.test(options.submissionKey)) {
    throw new Error('hosted runs require --submission-key to be a random UUID');
  }
}

function isObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function isUuid(value) {
  return typeof value === 'string' && UUID.test(value);
}

function validateRunIntent(value) {
  if (!isObject(value)) {
    throw new Error('Zero Cloud returned an invalid run intent');
  }
  if (!isUuid(value.intent_id)) {
    throw new Error('Zero Cloud returned an invalid run intent');
  }
  if (typeof value.state !== 'string' || !RUN_INTENT_STATES.has(value.state)) {
    throw new Error('Zero Cloud returned an invalid run intent');
  }
  if (value.result !== null && !isObject(value.result)) {
    throw new Error('Zero Cloud returned an invalid run intent result');
  }
  return value;
}

async function createContext(targetName, targetSession = null) {
  const session = targetSession || (await createHostedTargetSession(targetName));
  const authorization = await session.refresh();
  const organization = organizationFromToken(authorization.accessToken, session.organization);
  return {
    targetName,
    targetSession: session,
    endpoint: session.endpoint,
    organization,
    authorization,
  };
}

async function runIntentRequest(context, suffix, options = {}) {
  const pathname = `/api/v1/orgs/${context.organization}/run-intents${suffix}`;
  const send = () =>
    request(context.endpoint, pathname, {
      ...options,
      bearer: context.authorization.accessToken,
    });
  try {
    return validateRunIntent((await send()).body);
  } catch (error) {
    if (!(error instanceof HostedHttpError) || ![401, 403].includes(error.status)) throw error;
  }
  context.authorization = await context.targetSession.refresh();
  return validateRunIntent((await send()).body);
}

function displayState(intent) {
  return intent.waiting_reason ? `${intent.state} (${intent.waiting_reason})` : intent.state;
}

function resumeCommand(context, intentId) {
  return `zeroshot target status ${context.targetName} ${intentId} --follow`;
}

async function follow(context, initial) {
  let intent = initial;
  let displayed = null;
  for (;;) {
    const state = displayState(intent);
    if (state !== displayed) {
      console.log(`Run ${intent.intent_id}: ${state}`);
      displayed = state;
    }
    if (TERMINAL_STATES.has(intent.state)) break;
    await delay(RUN_INTENT_POLL_MS);
    intent = await runIntentRequest(context, `/${encodeURIComponent(intent.intent_id)}`);
  }
  if (intent.state === 'succeeded') {
    const summary = intent.result?.summary;
    if (typeof summary === 'string' && summary) console.log(summary);
    return intent.result;
  }
  const detail = intent.error_code ? ` (${intent.error_code})` : '';
  throw new Error(`hosted run ${intent.state}${detail}`);
}

async function run(input, options) {
  validateOptions(options);
  const targetSession = await createHostedTargetSession(options.target);
  const hostedRun = await buildHostedRun(input, targetSession.runtime, options);
  const context = await createContext(options.target, targetSession);
  const body = {
    label: 'zeroshot-cli',
    size: options.size || 'standard',
    intent: runIntentEnvelope(hostedRun.credentials, hostedRun.request),
  };
  if (Buffer.byteLength(JSON.stringify(body)) > MAX_RUN_INTENT_BYTES) {
    throw new Error('hosted run intent exceeds the 10 MiB upload limit');
  }
  const submissionKey = options.submissionKey || crypto.randomUUID();
  let created;
  try {
    created = await runIntentRequest(context, '', {
      method: 'POST',
      headers: { 'idempotency-key': submissionKey },
      json: body,
      accept: [202],
    });
  } catch (error) {
    if (error instanceof HostedHttpError && error.status < 500) throw error;
    throw new Error(
      `${error.message}. Recover this submission by rerunning the same command with ` +
        `--submission-key ${submissionKey}`,
      { cause: error }
    );
  }
  console.log(`Run ${created.intent_id} queued`);
  console.log(`Resume: ${resumeCommand(context, created.intent_id)}`);
  if (options.detach) return created;
  console.log('Ctrl+C disconnects without cancelling.');
  return follow(context, created);
}

async function status(targetName, intentId, shouldFollow) {
  if (!UUID.test(intentId)) throw new Error('run intent id must be a UUID');
  const context = await createContext(targetName);
  const intent = await runIntentRequest(context, `/${encodeURIComponent(intentId)}`);
  if (!shouldFollow) {
    console.log(JSON.stringify(intent, null, 2));
    return intent;
  }
  if (!TERMINAL_STATES.has(intent.state)) {
    console.log(`Following ${intentId}; Ctrl+C disconnects without cancelling.`);
    console.log(`Resume: ${resumeCommand(context, intentId)}`);
  }
  return follow(context, intent);
}

async function cancel(targetName, intentId) {
  if (!UUID.test(intentId)) throw new Error('run intent id must be a UUID');
  const context = await createContext(targetName);
  const intent = await runIntentRequest(context, `/${encodeURIComponent(intentId)}`, {
    method: 'DELETE',
    accept: [202],
  });
  console.log(`Run ${intent.intent_id}: ${displayState(intent)}`);
  return intent;
}

module.exports = Object.freeze({
  kind: 'queue',
  cancel,
  run,
  status,
  validateOptions,
  validateRunIntent,
});
