const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const buildOutput = path.join(__dirname, '..', '..', 'lib', 'tui', 'launcher-actions.js');

function ensureTuiBuild() {
  if (!fs.existsSync(buildOutput)) {
    execSync('npm run build:tui', { stdio: 'inherit' });
  }
}

ensureTuiBuild();

const { submitLauncherText } = require('../../lib/tui/launcher-actions');

function createHarness() {
  const calls = {
    status: [],
    clusterIds: [],
    navigate: [],
  };

  return {
    calls,
    setStatus: (status) => calls.status.push(status),
    setClusterId: (clusterId) => calls.clusterIds.push(clusterId),
    navigate: (view) => calls.navigate.push(view),
  };
}

describe('TUI launcher actions', function () {
  it('starts a cluster from text and navigates', async function () {
    const { calls, setStatus, setClusterId, navigate } = createHarness();
    const launchCalls = [];
    const deps = {
      generateClusterId: () => 'cluster-test-1',
      launchClusterFromText: (options) => {
        launchCalls.push(options);
      },
    };

    await submitLauncherText({
      text: 'Implement X',
      providerOverride: 'codex',
      activeView: 'launcher',
      setStatus,
      setClusterId,
      navigate,
      deps,
    });

    assert.strictEqual(launchCalls.length, 1);
    assert.strictEqual(launchCalls[0].text, 'Implement X');
    assert.strictEqual(launchCalls[0].providerOverride, 'codex');
    assert.strictEqual(launchCalls[0].clusterId, 'cluster-test-1');
    assert.deepStrictEqual(calls.navigate, ['cluster']);
    assert.ok(calls.status.some((status) => status.message.includes('cluster-test-1')));
    assert.strictEqual(calls.clusterIds[0], 'cluster-test-1');
  });

  it('treats numeric input as plain text', async function () {
    const { calls, setStatus, setClusterId, navigate } = createHarness();
    const launchCalls = [];
    const deps = {
      generateClusterId: () => 'cluster-test-2',
      launchClusterFromText: (options) => {
        launchCalls.push(options);
      },
    };

    await submitLauncherText({
      text: '123',
      providerOverride: null,
      activeView: 'launcher',
      setStatus,
      setClusterId,
      navigate,
      deps,
    });

    assert.strictEqual(launchCalls.length, 1);
    assert.strictEqual(launchCalls[0].text, '123');
    assert.strictEqual(launchCalls[0].clusterId, 'cluster-test-2');
    assert.deepStrictEqual(calls.navigate, ['cluster']);
  });

  it('shows error and stays on launcher when start fails', async function () {
    const { calls, setStatus, setClusterId, navigate } = createHarness();
    const deps = {
      generateClusterId: () => 'cluster-fail',
      launchClusterFromText: () => {
        throw new Error('boom');
      },
    };

    await submitLauncherText({
      text: 'Fail me',
      providerOverride: null,
      activeView: 'launcher',
      setStatus,
      setClusterId,
      navigate,
      deps,
    });

    assert.deepStrictEqual(calls.navigate, []);
    assert.strictEqual(calls.clusterIds[0], 'cluster-fail');
    assert.strictEqual(calls.clusterIds[calls.clusterIds.length - 1], null);
    const lastStatus = calls.status[calls.status.length - 1];
    assert.strictEqual(lastStatus.tone, 'error');
    assert.ok(lastStatus.message.includes('boom'));
  });
});
