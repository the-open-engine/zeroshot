'use strict';

const { loadSettings, mutateSettings } = require('../../lib/settings');

const PROCESS_REFRESH_TOKEN_ENV = 'ZEROSHOT_TARGET_REFRESH_TOKEN';

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
  const processRefreshToken = environment[PROCESS_REFRESH_TOKEN_ENV];
  const credentialStore =
    typeof processRefreshToken === 'string' && processRefreshToken.trim()
      ? new ProcessRefreshTokenStore(processRefreshToken)
      : await runtime.KeyringCredentialStore.create();

  return {
    endpoint: target.url,
    organization: target.organization?.id || null,
    runtime: target.runtime || null,
    refresh() {
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
  PROCESS_REFRESH_TOKEN_ENV,
  ProcessRefreshTokenStore,
  createHostedTargetSession,
  loadTargetRuntime,
};
