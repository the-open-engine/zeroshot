'use strict';

const assert = require('node:assert').strict;
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { describe, it } = require('mocha');

const {
  credentialsForRun,
  graph,
  issueInput,
  resolveInput,
  validateHostedOptions,
  websocketUrl,
} = require('../../cli/hosted/run');
const { normalizeRuntimeConfig, readRuntimeConfig } = require('../../cli/hosted/runtime-config');
const {
  ProcessRefreshTokenStore,
  createHostedTargetSession,
} = require('../../cli/hosted/target-session');

function fixture() {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-hosted-target-'));
  return {
    directory,
  };
}

function runtimeConfig() {
  return {
    provider: 'gemini',
    model: 'gemini-2.5-pro',
    command: 'npx --yes @google/gemini-cli',
    setupCommand: 'npm --version',
    environment: {
      GEMINI_API_KEY: { from: 'LOCAL_MODEL_KEY' },
      MODEL_ENDPOINT: 'https://models.example',
    },
    files: {
      '.config/harness.json': '{"enabled":true}',
    },
    settings: {
      providerSettings: { gemini: { defaultLevel: 'level2' } },
    },
  };
}

describe('hosted target CLI', function () {
  it('keeps the automation refresh token process-local while accepting rotation', async function () {
    const store = new ProcessRefreshTokenStore('initial-refresh');
    assert.equal(await store.get(), 'initial-refresh');
    await store.set('ignored-service', 'ignored-account', 'rotated-refresh');
    assert.equal(await store.get(), 'rotated-refresh');
    await store.delete();
    assert.equal(await store.get(), null);
  });

  it('uses upstream target metadata and session rotation for hosted capsule authority', async function () {
    const environment = { ZEROSHOT_TARGET_REFRESH_TOKEN: 'process-refresh' };
    const target = {
      id: 'target-id',
      url: 'http://127.0.0.1:8080',
      organization: { id: 'organization-id', name: 'Local' },
      runtime: runtimeConfig(),
    };
    let observedAudience = null;
    const runtime = {
      getTarget(name) {
        assert.equal(name, 'local');
        return target;
      },
      discoverTargetSessionEndpoints(url) {
        assert.equal(url, target.url);
        return Promise.resolve({ tokenEndpoint: `${url}/auth/token`, clientId: 'cli' });
      },
      KeyringCredentialStore: {
        create() {
          throw new Error('process automation must not open the keyring');
        },
      },
      acquireTargetLock(id) {
        assert.equal(id, target.id);
        return Promise.resolve(async () => {});
      },
      async refreshAccessToken(_name, _target, store, acquireLock, deps) {
        observedAudience = deps.audience;
        const release = await acquireLock();
        assert.equal(await store.get(), 'process-refresh');
        await store.set('service', 'account', 'rotated-refresh');
        await release();
        return { accessToken: 'capsule-access', expiresIn: 300 };
      },
    };

    const session = await createHostedTargetSession('local', {
      environment,
      runtime,
      settingsPort: { load: () => ({}), mutate() {} },
      http: { fetch: () => Promise.reject(new Error('unused')) },
    });
    assert.equal(session.endpoint, target.url);
    assert.equal(session.organization, 'organization-id');
    assert.deepEqual(session.runtime, runtimeConfig());
    assert.deepEqual(await session.refresh(), { accessToken: 'capsule-access', expiresIn: 300 });
    assert.equal(observedAudience, 'capsule');
    assert.equal(environment.ZEROSHOT_TARGET_REFRESH_TOKEN, 'process-refresh');
  });

  it('rejects a legacy target before discovery or credential access', async function () {
    let discoveryAttempted = false;
    await assert.rejects(
      createHostedTargetSession('legacy', {
        runtime: {
          getTarget: () => ({ id: 'target-id', url: 'https://cloud.example' }),
          discoverTargetSessionEndpoints: () => {
            discoveryAttempted = true;
            throw new Error('must not discover');
          },
          KeyringCredentialStore: {
            create: () => {
              throw new Error('must not open keyring');
            },
          },
        },
        settingsPort: { load: () => ({}), mutate() {} },
      }),
      /has no runtime config; re-add it with --runtime-config/
    );
    assert.equal(discoveryAttempted, false);
  });

  it('uses the canonical single-worker facade for hosted issues', function () {
    const issue = issueInput('the-open-engine/zeroshot#837');
    assert.equal(issue.repository, 'the-open-engine/zeroshot');
    assert.equal(issue.request.issue, 'https://github.com/the-open-engine/zeroshot/issues/837');
    assert.ok(!Object.hasOwn(issue.request, 'prompt'));
    const specification = graph();
    assert.equal(specification.profile, 'openengine.graph.single-worker/v1');
    assert.equal(specification.root.worker, 'legacy.zeroshot.ship@1');
    assert.equal(specification.root.input.fields.providerProfile.required, true);
    assert.equal(specification.root.output.fields.summary.type.kind, 'string');
  });

  it('selects reviewed PR delivery only when the hosted run requests it', async function () {
    assert.equal(
      (await resolveInput('the-open-engine/zeroshot#837', {})).request.isolationProfile,
      'isolation.worktree@1'
    );
    const issuePr = await resolveInput('the-open-engine/zeroshot#837', { pr: true });
    assert.equal(issuePr.request.isolationProfile, 'isolation.pr@1');
    assert.equal(issuePr.request.providerProfile, 'provider.hosted-pr@1');
    assert.equal(
      (
        await resolveInput('Implement the issue', {
          pr: true,
          repository: 'the-open-engine/zeroshot',
        })
      ).request.isolationProfile,
      'isolation.pr@1'
    );
  });

  it('reads a hosted prompt from one opened regular file', async function () {
    const { directory } = fixture();
    const prompt = path.join(directory, 'prompt.md');
    try {
      fs.writeFileSync(prompt, 'Implement the focused task.\n');
      const resolved = await resolveInput(prompt, { repository: 'the-open-engine/zeroshot' });
      assert.equal(resolved.request.prompt, 'Implement the focused task.');
    } finally {
      fs.rmSync(directory, { recursive: true, force: true });
    }
  });

  it('rejects repository path segments that Git would normalize', function () {
    assert.equal(issueInput('../zeroshot#837'), null);
    assert.equal(issueInput('the-open-engine/..#837'), null);
    assert.equal(issueInput('https://github.com/../zeroshot/issues/837'), null);
  });

  it('routes localhost advertised wss access through the target', function () {
    assert.equal(
      websocketUrl(
        { endpoint: 'http://127.0.0.1:49152' },
        'wss://capsule.localtest.me/v1/capsules/example/oecp'
      ),
      'ws://127.0.0.1:49152/v1/capsules/example/oecp'
    );
  });

  it('rejects local-only flags instead of silently ignoring them', function () {
    assert.doesNotThrow(() => validateHostedOptions({ target: 'local', model: 'local-model' }));
    assert.doesNotThrow(() => validateHostedOptions({ target: 'local', pr: true }));
    assert.doesNotThrow(() => validateHostedOptions({ target: 'local', provider: 'gemini' }));
    assert.doesNotThrow(() => validateHostedOptions({ target: 'local', size: 'standard' }));
    assert.throws(() => validateHostedOptions({ target: 'local', docker: true }), /--docker/);
    assert.throws(
      () => validateHostedOptions({ target: 'local', size: 'xlarge' }),
      /--size tiny, small, standard, or large/
    );
  });

  it('resolves arbitrary target environment and files into one generic runtime bundle', function () {
    const credentials = credentialsForRun(
      { repository: 'the-open-engine/zeroshot' },
      runtimeConfig(),
      {},
      { GH_TOKEN: 'github-test-token', LOCAL_MODEL_KEY: 'model-test-token' }
    );

    assert.equal(credentials.githubToken, 'github-test-token');
    assert.equal(credentials.runtime.provider, 'gemini');
    assert.equal(credentials.runtime.executable, 'gemini');
    assert.equal(credentials.runtime.model, 'gemini-2.5-pro');
    assert.equal(credentials.runtime.environment.GEMINI_API_KEY, 'model-test-token');
    assert.equal(credentials.runtime.environment.MODEL_ENDPOINT, 'https://models.example');
    assert.equal(credentials.runtime.files['.config/harness.json'], '{"enabled":true}');
    assert.deepEqual(Object.keys(credentials.runtime.settings.providerSettings), ['gemini']);
    assert.ok(!JSON.stringify(credentials).includes('openrouterApiKey'));

    const providerOverride = credentialsForRun(
      { repository: 'the-open-engine/zeroshot' },
      runtimeConfig(),
      { provider: 'claude' },
      { GH_TOKEN: 'github-test-token', LOCAL_MODEL_KEY: 'model-test-token' }
    );
    assert.equal(providerOverride.runtime.provider, 'claude');
    assert.equal(providerOverride.runtime.executable, 'claude');
    assert.equal(providerOverride.runtime.model, undefined);
    assert.equal(providerOverride.runtime.command, undefined);
    assert.deepEqual(Object.keys(providerOverride.runtime.settings.providerSettings), ['claude']);
  });

  it('validates target runtime mappings and anchors mapped files to the config', function () {
    assert.deepEqual(normalizeRuntimeConfig(runtimeConfig()), runtimeConfig());
    assert.throws(
      () => normalizeRuntimeConfig({ ...runtimeConfig(), environment: { HOME: '/escape' } }),
      /reserved by Zero Cloud/
    );
    assert.throws(
      () => normalizeRuntimeConfig({ ...runtimeConfig(), files: { '../escape': 'secret' } }),
      /runtime file path/
    );

    const { directory } = fixture();
    const configDirectory = path.join(directory, 'config');
    const configFile = path.join(configDirectory, 'runtime.json');
    try {
      fs.mkdirSync(configDirectory);
      fs.writeFileSync(
        configFile,
        JSON.stringify({
          ...runtimeConfig(),
          files: { '.config/harness.json': { from: '../credentials/harness.json' } },
        })
      );
      assert.equal(
        readRuntimeConfig(configFile).files['.config/harness.json'].from,
        path.join(directory, 'credentials', 'harness.json')
      );
    } finally {
      fs.rmSync(directory, { recursive: true, force: true });
    }
  });
});
