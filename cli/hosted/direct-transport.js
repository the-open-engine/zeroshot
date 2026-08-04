'use strict';

const crypto = require('node:crypto');
const { URL } = require('node:url');

const { buildHostedRun, validateHostedOptions } = require('./contract');
const { HostedHttpError, request } = require('./http');
const { createHostedTargetSession, organizationFromToken } = require('./target-session');

const PROTOCOL = 'openengine.cluster/v1';
const WORKER = 'legacy.zeroshot.ship@1';
const READY_TIMEOUT_MS = 10 * 60 * 1_000;

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

async function waitForReady(target, organization, capsuleId, accessToken) {
  const pathname = `/api/v1/orgs/${organization}/capsules/${capsuleId}`;
  const deadline = Date.now() + READY_TIMEOUT_MS;
  let lastState = 'provisioning';
  while (Date.now() < deadline) {
    const capsule = (await request(target.endpoint, pathname, { bearer: accessToken })).body;
    lastState = capsule?.state;
    if (lastState === 'ready') return;
    if (['failed', 'terminated'].includes(lastState)) {
      throw new Error(`capsule entered terminal state ${lastState} before it was ready`);
    }
    await delay(500);
  }
  throw new Error(`capsule did not become ready; last state was ${lastState}`);
}

async function mintAccess(target, capsuleId, accessToken) {
  const pathname = `/api/v1/capsules/${capsuleId}/access`;
  const deadline = Date.now() + READY_TIMEOUT_MS;
  while (Date.now() < deadline) {
    try {
      return (
        await request(target.endpoint, pathname, {
          method: 'POST',
          bearer: accessToken,
          json: { protocol: PROTOCOL },
        })
      ).body;
    } catch (error) {
      if (!(error instanceof HostedHttpError) || error.code !== 'capsule_not_ready') throw error;
      await delay(500);
    }
  }
  throw new Error('capsule access did not become ready');
}

function websocketUrl(target, advertised) {
  const url = new URL(advertised);
  if (new URL(target.endpoint).protocol === 'http:') {
    const local = new URL(target.endpoint);
    url.protocol = 'ws:';
    url.host = local.host;
  }
  return url.toString();
}

function schemaDefinition(name) {
  const schema = require('../../protocol/openengine-cluster/v1/worker.schema.json');
  return payloadType(schema.$defs[name], schema.$defs);
}

function payloadType(schema, definitions) {
  if (schema.$ref) {
    const name = schema.$ref.split('/').pop();
    return payloadType(definitions[name], definitions);
  }
  if (schema.enum) return { kind: 'enum', values: [...schema.enum].sort() };
  const type = Array.isArray(schema.type)
    ? schema.type.find((item) => item !== 'null')
    : schema.type;
  if (type === 'object') {
    const required = new Set(schema.required || []);
    const fields = {};
    for (const name of Object.keys(schema.properties || {}).sort()) {
      fields[name] = {
        type: payloadType(schema.properties[name], definitions),
        required: required.has(name),
      };
    }
    return { kind: 'record', fields };
  }
  if (type === 'array') return { kind: 'array', items: payloadType(schema.items, definitions) };
  if (['null', 'boolean', 'integer', 'number', 'string'].includes(type)) return { kind: type };
  throw new Error('installed OECP worker schema cannot be converted to a graph contract');
}

function graph() {
  const input = schemaDefinition('LegacyShipRequest');
  const output = schemaDefinition('LegacyShipResult');
  return {
    initialInput: input,
    policy: { default: 'deny', policy: 'policy.strict@1' },
    profile: 'openengine.graph.single-worker/v1',
    root: {
      attempts: 1,
      input,
      inputBindings: [],
      kind: 'step',
      name: 'zeroshot',
      output,
      timeoutMs: 60 * 60 * 1_000,
      worker: WORKER,
      writeBindings: [],
    },
  };
}

function clusterLibrary() {
  try {
    return require('../../lib/cluster/index.cjs');
  } catch (error) {
    if (error?.code === 'MODULE_NOT_FOUND') {
      throw new Error('OECP client is not built; run npm run build:cluster');
    }
    throw error;
  }
}

async function watchRun(client, runId) {
  const watch = await client.watch({ runId });
  let outcome = null;
  for await (const item of watch.stream) {
    if (item.type === 'closed') throw new Error(`OECP watch closed early: ${item.reason}`);
    const event = item.event;
    if (event.type === 'node_begin') console.log(`Worker ${event.node.node} started`);
    if (event.type === 'node_end') outcome = event.outcome;
    if (event.type === 'finished') break;
  }
  await watch.stream.cancel();
  return outcome;
}

async function applyRun(client, requestInput) {
  const specification = graph();
  const planned = await client.plan({ graph: specification });
  if (!planned.ok) {
    const detail = planned.diagnostics.map((item) => item.message).join('; ');
    throw new Error(`OECP plan rejected the hosted graph${detail ? `: ${detail}` : ''}`);
  }
  const applied = await client.apply({
    graph: specification,
    input: requestInput,
    idempotencyKey: crypto.randomUUID(),
    ifGeneration: 0,
  });
  if (!applied.runId) throw new Error('OECP apply returned no run id');
  console.log(`Run ${applied.runId} admitted`);
  const outcome = await watchRun(client, applied.runId);
  const final = await client.get({});
  if (final.status.phase !== 'finished') throw new Error('OECP run did not reach finished');
  if (!outcome) throw new Error('OECP run finished without a worker outcome');
  if (outcome.status !== 'verified') {
    throw new Error(`hosted worker failed (${outcome.code || outcome.status})`);
  }
  const result = outcome.output;
  if (result?.summary) console.log(result.summary);
  return result;
}

async function executeOecp(target, grant, requestInput) {
  const WebSocket = require('ws');
  const { ClusterClient, connect } = clusterLibrary();
  const connection = await connect(websocketUrl(target, grant.websocket_url), {
    webSocketFactory: (url, protocols) =>
      new WebSocket(url, protocols || [], {
        headers: { authorization: `Bearer ${grant.access_token}` },
        perMessageDeflate: false,
      }),
  });
  try {
    return await applyRun(new ClusterClient(connection), requestInput);
  } catch (error) {
    const reason = error?.data?.details?.reason;
    if (typeof reason === 'string' && reason) {
      throw new Error(`${error.message}: ${reason}`, { cause: error });
    }
    throw error;
  } finally {
    await connection.close();
  }
}

async function terminateCapsule(target, organization, capsuleId, targetSession, authorization) {
  const pathname = `/api/v1/orgs/${organization}/capsules/${capsuleId}`;
  try {
    await request(target.endpoint, pathname, {
      method: 'DELETE',
      bearer: authorization.accessToken,
      accept: [202],
    });
    return authorization;
  } catch (error) {
    if (!(error instanceof HostedHttpError) || ![401, 403].includes(error.status)) throw error;
  }
  const rotated = await targetSession.refresh();
  await request(target.endpoint, pathname, {
    method: 'DELETE',
    bearer: rotated.accessToken,
    accept: [202],
  });
  return rotated;
}

async function run(input, options) {
  validateHostedOptions(options);
  const targetName = options.target;
  const targetSession = await createHostedTargetSession(targetName);
  const target = { endpoint: targetSession.endpoint };
  const hostedRun = await buildHostedRun(input, targetSession.runtime, options);
  let authorization = await targetSession.refresh();
  const organization = organizationFromToken(authorization.accessToken, targetSession.organization);
  let capsuleId = null;
  try {
    const created = (
      await request(target.endpoint, `/api/v1/orgs/${organization}/capsules`, {
        method: 'POST',
        bearer: authorization.accessToken,
        headers: { 'idempotency-key': crypto.randomUUID() },
        json: { label: 'zeroshot-cli', size: options.size || 'standard' },
        accept: [201],
      })
    ).body;
    capsuleId = created?.capsule_id;
    if (typeof capsuleId !== 'string') throw new Error('Zero Cloud returned no capsule id');
    console.log(`Capsule ${capsuleId} provisioning`);
    await waitForReady(target, organization, capsuleId, authorization.accessToken);
    console.log(`Capsule ${capsuleId} ready`);
    const grant = await mintAccess(target, capsuleId, authorization.accessToken);
    await request(target.endpoint, `/api/v1/capsules/${capsuleId}/credentials`, {
      method: 'PUT',
      bearer: grant.access_token,
      json: hostedRun.credentials,
      accept: [204],
    });
    return await executeOecp(target, grant, hostedRun.request);
  } finally {
    if (capsuleId) {
      try {
        await terminateCapsule(target, organization, capsuleId, targetSession, authorization);
        console.log(`Capsule ${capsuleId} termination requested`);
      } catch (error) {
        console.error(`Warning: capsule cleanup failed: ${error.message}`);
      }
    }
  }
}

module.exports = Object.freeze({
  kind: 'direct',
  graph,
  payloadType,
  run,
  websocketUrl,
});
