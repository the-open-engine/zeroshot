'use strict';

const assert = require('node:assert').strict;
const { describe, it } = require('mocha');

const { runIntentEnvelope, validateHostedQueueOptions } = require('../../cli/hosted/run-intent');

describe('hosted run intent CLI', function () {
  it('accepts detach and requires a random submission UUID', function () {
    assert.doesNotThrow(() => validateHostedQueueOptions({ target: 'local', detach: true }));
    assert.doesNotThrow(() =>
      validateHostedQueueOptions({
        target: 'local',
        submissionKey: '019f7437-8701-41e3-a056-2ba05c37609c',
      })
    );
    assert.throws(
      () => validateHostedQueueOptions({ target: 'local', submissionKey: 'predictable' }),
      /random UUID/
    );
  });

  it('wraps the exact generic direct-credential input without provider interpretation', function () {
    const credentials = {
      githubToken: 'github-test-token',
      repository: 'the-open-engine/zeroshot',
      runtime: {
        provider: 'gemini',
        executable: 'gemini',
        model: 'gemini-2.5-pro',
        environment: { GEMINI_API_KEY: 'model-test-token' },
        files: {},
        settings: { defaultProvider: 'gemini', providerSettings: { gemini: {} } },
      },
    };
    const request = {
      source: 'prompt',
      prompt: 'exercise the durable queue',
      artifacts: [],
      isolationProfile: 'isolation.worktree@1',
      providerProfile: 'provider.hosted@1',
    };
    const intent = runIntentEnvelope(credentials, request);

    assert.deepEqual(intent, {
      version: 'zeroshot.run-intent/v1',
      credentials,
      request,
    });
    assert.ok(!JSON.stringify(intent).includes('openrouterApiKey'));
  });
});
