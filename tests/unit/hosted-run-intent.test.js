'use strict';

const assert = require('node:assert').strict;
const fs = require('node:fs');
const path = require('node:path');
const { describe, it } = require('mocha');

const { runIntentEnvelope } = require('../../cli/hosted/contract');
const queueTransport = require('../../cli/hosted/queue-transport');

const golden = require(path.join('..', 'fixtures', 'hosted', 'run-intent-v1.json'));

describe('hosted run intent CLI', function () {
  it('accepts detach and requires a random submission UUID', function () {
    assert.doesNotThrow(() => queueTransport.validateOptions({ target: 'local', detach: true }));
    assert.doesNotThrow(() =>
      queueTransport.validateOptions({
        target: 'local',
        submissionKey: '019f7437-8701-41e3-a056-2ba05c37609c',
      })
    );
    assert.throws(
      () => queueTransport.validateOptions({ target: 'local', submissionKey: 'predictable' }),
      /random UUID/
    );
  });

  it('wraps the exact generic direct-credential input without provider interpretation', function () {
    const { credentials, request } = golden.envelope;
    const intent = runIntentEnvelope(credentials, request);

    assert.deepEqual(intent, golden.envelope);
    assert.ok(!JSON.stringify(intent).includes('openrouterApiKey'));
  });

  it('selects an explicit hosted transport and keeps queue as the only active default', function () {
    const selectorPath = require.resolve('../../cli/hosted/run');
    const directPath = require.resolve('../../cli/hosted/direct-transport');
    delete require.cache[selectorPath];
    delete require.cache[directPath];
    const { DEFAULT_HOSTED_TRANSPORT, selectHostedTransport } = require(selectorPath);

    assert.equal(DEFAULT_HOSTED_TRANSPORT, 'queue');
    assert.equal(selectHostedTransport().kind, 'queue');
    assert.equal(require.cache[directPath], undefined);
    assert.equal(selectHostedTransport('direct').kind, 'direct');
    assert.throws(() => selectHostedTransport('unknown'), /unknown hosted transport/);
  });

  it('keeps the contract and transport adapters independent', function () {
    const source = (name) => fs.readFileSync(require.resolve(`../../cli/hosted/${name}`), 'utf8');

    assert.doesNotMatch(source('contract'), /(?:queue|direct)-transport/);
    assert.doesNotMatch(source('queue-transport'), /direct-transport/);
    assert.doesNotMatch(source('direct-transport'), /queue-transport/);
  });
});
