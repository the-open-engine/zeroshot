'use strict';

const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { createCurrentEngineAdapter } = require('../../lib/cluster-worker/engine-adapter');
const { registry } = require('./helpers');

describe('legacy cluster worker engine adapter artifact staging', () => {
  it('stages artifact input in allocated isolation before a fake agent reads it', async () => {
    const isolationRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'cluster-worker-artifact-'));
    const messageBus = Object.freeze({ subscribe: () => () => {}, getAll: () => [] });
    const cluster = { state: 'running', ledger: { count: () => 0 } };
    let fakeAgentRead;
    const orchestrator = {
      async start(_config, input, options) {
        assert.match(input.text, /artifact input is being prepared/i);
        const preparedText = await options.prepareIsolatedInput({
          clusterId: 'cluster-artifact',
          isolation: Object.freeze({
            kind: 'worktree',
            hostRoot: isolationRoot,
            runtimeRoot: isolationRoot,
          }),
        });
        const manifest = JSON.parse(preparedText.slice(preparedText.indexOf('{')));
        fakeAgentRead = fs.readFileSync(manifest.artifacts[0].path, 'utf8');
        return { id: 'cluster-artifact', messageBus };
      },
      getCluster() {
        return cluster;
      },
      getStatus() {
        return { isZombie: false };
      },
      stop() {
        cluster.state = 'stopped';
      },
      close() {},
    };
    const adapter = createCurrentEngineAdapter({
      orchestrator,
      startCluster: {
        buildTrustedStartOptions: ({ clusterId }) => ({ clusterId, worktree: true }),
        resolveConfigPath: (name) => name,
        loadClusterConfig: () => ({ agents: [] }),
      },
    });

    try {
      const result = await adapter.start({
        request: { source: 'artifact', artifacts: [{ artifactId: 'artifact-1' }] },
        profile: registry({ shutdownMs: 50 }).resolve('isolation.worktree@1', 'provider.default@1'),
        prepareArtifacts(isolation) {
          const artifactPath = path.join(isolation.hostRoot, 'artifact-1.txt');
          fs.writeFileSync(artifactPath, 'resolved artifact bytes', { mode: 0o400 });
          fs.chmodSync(artifactPath, 0o400);
          return { artifacts: [{ artifactId: 'artifact-1', path: artifactPath }] };
        },
        clusterId: 'cluster-artifact',
        onEvent() {},
      });

      assert.strictEqual(result.artifactsStaged, true);
      assert.strictEqual(fakeAgentRead, 'resolved artifact bytes');
    } finally {
      fs.rmSync(isolationRoot, { recursive: true, force: true });
    }
  });
});
