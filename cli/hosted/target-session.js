'use strict';

const { loadSettings, mutateSettings } = require('../../lib/settings');

const PROCESS_REFRESH_TOKEN_ENV = 'ZEROSHOT_TARGET_REFRESH_TOKEN';
const PROCESS_ACCESS_TOKEN_ENV = 'ZEROSHOT_TARGET_ACCESS_TOKEN';
const PROCESS_ORGANIZATION_ENV = 'ZEROSHOT_TARGET_ORGANIZATION';

class ProcessRefreshTokenStore {
  #token;

  constructor(token) {
    if (typeof token !== 'string' || !token.trim()) {
      throw new Error(`${PROCESS_REFRESH_TOKEN_ENV} must contain a refresh token`);
    }
    this.#token = token.trim();
  }

  get() {
    return Promise.resolve(this.#token);
  }

  set(_service, _account, token) {
    if (typeof token !== 'string' || !token) {
      throw new Error('Zero Cloud returned an empty refresh token');
    }
    this.#token = token;
    return Promise.resolve();
  }

  delete() {
    this.#token = null;
    return Promise.resolve();
  }
}

function loadTargetRuntime() {
  try {
    return {
      ...require('../../lib/target/credential-lock'),
      ...require('../../lib/target/credential-store'),
      ...require('../../lib/target/discovery'),
      ...require('../../lib/target/target-registry'),
      ...require('../../lib/target/target-session'),
    };
  } catch (error) {
    if (error?.code === 'MODULE_NOT_FOUND') {
      throw new Error('hosted target client is not built; run npm run build:target');
    }
    throw error;
  }
}

function defaultSettingsPort() {
  return {
    load: () => loadSettings(),
    mutate: (mutator) => mutateSettings(mutator),
  };
}

function processAccessAuthority(environment) {
  const accessToken = environment[PROCESS_ACCESS_TOKEN_ENV]?.trim();
  const organization = environment[PROCESS_ORGANIZATION_ENV]?.trim();
  if ((accessToken && !organization) || (!accessToken && organization)) {
    throw new Error(
      `${PROCESS_ACCESS_TOKEN_ENV} and ${PROCESS_ORGANIZATION_ENV} must be provided together`
    );
  }
  return accessToken ? { accessToken, organization } : null;
}

function organizationFromToken(token, expectedOrganization = null) {
  const segments = token.split('.');
  if (segments.length !== 3) throw new Error('Zero Cloud returned an invalid access token');
  let claims;
  try {
    claims = JSON.parse(Buffer.from(segments[1], 'base64url').toString('utf8'));
  } catch {
    throw new Error('Zero Cloud returned an invalid access token');
  }
  if (typeof claims.org_id !== 'string' || !/^[0-9a-f-]{36}$/i.test(claims.org_id)) {
    throw new Error('target login is not bound to an organization');
  }
  if (expectedOrganization && claims.org_id !== expectedOrganization) {
    throw new Error('target login organization does not match the configured target');
  }
  return expectedOrganization || claims.org_id;
}

function credentialStoreFor(runtime, environment, processAccess) {
  if (processAccess) return null;
  const processRefreshToken = environment[PROCESS_REFRESH_TOKEN_ENV];
  return typeof processRefreshToken === 'string' && processRefreshToken.trim()
    ? new ProcessRefreshTokenStore(processRefreshToken)
    : runtime.KeyringCredentialStore.create();
}

async function createHostedTargetSession(targetName, options = {}) {
  const environment = options.environment || process.env;
  const runtime = options.runtime || loadTargetRuntime();
  const settingsPort = options.settingsPort || defaultSettingsPort();
  const target = runtime.getTarget(targetName, settingsPort);
  if (!target) throw new Error(`Target "${targetName}" not found.`);
  if (!target.runtime) {
    throw new Error(
      `target ${targetName} has no runtime config; re-add it with --runtime-config <file>`
    );
  }

  const http = options.http || { fetch: (url, init) => fetch(url, init) };
  const discoveryEndpoints = await runtime.discoverTargetSessionEndpoints(target.url, http);
  const processAccess = processAccessAuthority(environment);
  const credentialStore = await credentialStoreFor(runtime, environment, processAccess);

  return {
    endpoint: target.url,
    organization: processAccess?.organization || target.organization?.id || null,
    runtime: target.runtime || null,
    refresh() {
      if (processAccess) {
        return Promise.resolve({ accessToken: processAccess.accessToken, expiresIn: 0 });
      }
      return runtime.refreshAccessToken(
        targetName,
        target,
        credentialStore,
        () => runtime.acquireTargetLock(target.id),
        { http, discoveryEndpoints, audience: 'capsule' }
      );
    },
  };
}

module.exports = {
  PROCESS_ACCESS_TOKEN_ENV,
  PROCESS_ORGANIZATION_ENV,
  PROCESS_REFRESH_TOKEN_ENV,
  ProcessRefreshTokenStore,
  createHostedTargetSession,
  loadTargetRuntime,
  organizationFromToken,
};
