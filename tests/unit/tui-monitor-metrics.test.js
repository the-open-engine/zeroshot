const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const buildOutput = path.join(
  __dirname,
  '..',
  '..',
  'lib',
  'tui',
  'services',
  'monitor-metrics.js'
);

function ensureTuiBuild() {
  if (!fs.existsSync(buildOutput)) {
    execSync('npm run build:tui', { stdio: 'inherit' });
  }
}

ensureTuiBuild();

const { fetchMonitorMetrics } = require('../../lib/tui/services/monitor-metrics');

describe('TUI monitor metrics', function () {
  it('aggregates cpu and memory for running clusters', async function () {
    const orchestrator = {
      listClusters: () => [
        {
          id: 'cluster-a',
          state: 'running',
          createdAt: 100,
          agentCount: 2,
          messageCount: 12,
        },
        {
          id: 'cluster-b',
          state: 'completed',
          createdAt: 200,
          agentCount: 1,
          messageCount: 4,
        },
      ],
      getStatus: (id) => {
        if (id === 'cluster-a') {
          return { agents: [{ pid: 101 }, { pid: 202 }] };
        }
        return { agents: [] };
      },
    };

    const pidusage = (pids) => {
      if (Array.isArray(pids)) {
        return {
          101: { cpu: 12.5, memory: 2560 },
          202: { cpu: 7.5, memory: 1024 },
        };
      }
      return { cpu: 0, memory: 0 };
    };

    const result = await fetchMonitorMetrics({
      getOrchestrator: () => Promise.resolve(orchestrator),
      pidusage,
      platform: 'darwin',
    });

    assert.strictEqual(result.error, null);
    const rowA = result.rows.find((row) => row.id === 'cluster-a');
    const rowB = result.rows.find((row) => row.id === 'cluster-b');
    assert.ok(rowA);
    assert.ok(rowB);
    assert.strictEqual(rowA.cpu, 20);
    assert.strictEqual(rowA.memory, 3584);
    assert.strictEqual(rowB.cpu, null);
    assert.strictEqual(rowB.memory, null);
  });

  it('returns placeholders when pidusage fails', async function () {
    const orchestrator = {
      listClusters: () => [
        {
          id: 'cluster-error',
          state: 'running',
          createdAt: 300,
          agentCount: 1,
          messageCount: 1,
        },
      ],
      getStatus: () => ({ agents: [{ pid: 999 }] }),
    };

    const pidusage = () => {
      throw new Error('pidusage failed');
    };

    const result = await fetchMonitorMetrics({
      getOrchestrator: () => Promise.resolve(orchestrator),
      pidusage,
      platform: 'linux',
    });

    assert.ok(result.error);
    assert.ok(result.error.includes('pidusage failed'));
    assert.strictEqual(result.rows[0].cpu, null);
    assert.strictEqual(result.rows[0].memory, null);
  });

  it('skips metrics on unsupported platforms', async function () {
    let called = false;
    const orchestrator = {
      listClusters: () => [
        {
          id: 'cluster-win',
          state: 'running',
          createdAt: 400,
          agentCount: 1,
          messageCount: 2,
        },
      ],
      getStatus: () => ({ agents: [{ pid: 111 }] }),
    };

    const pidusage = () => {
      called = true;
      return {};
    };

    const result = await fetchMonitorMetrics({
      getOrchestrator: () => Promise.resolve(orchestrator),
      pidusage,
      platform: 'win32',
    });

    assert.strictEqual(called, false);
    assert.ok(result.error);
    assert.ok(result.error.includes('win32'));
    assert.strictEqual(result.rows[0].cpu, null);
    assert.strictEqual(result.rows[0].memory, null);
  });
});
