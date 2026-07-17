'use strict';

const assert = require('assert');
const { LegacyWorkerStateMachine } = require('../../lib/cluster-worker/state-machine');

function completed(clusterId = 'cluster-1') {
  return {
    state: 'completed',
    clusterId,
    finishedAt: 3,
    result: { summary: 'done', status: 'succeeded', artifacts: [] },
  };
}

describe('legacy cluster worker state machine', () => {
  it('emits live monotonic events and closes streams after terminal state', async () => {
    const machine = new LegacyWorkerStateMachine({ clock: () => 2 });
    machine.setClusterId('cluster-1');
    const events = machine.events();
    machine.transition('starting');
    machine.transition('running');
    machine.terminal(completed());
    assert.deepStrictEqual((await events.next()).value, { sequence: 1, state: 'starting', at: 2 });
    assert.deepStrictEqual((await events.next()).value, { sequence: 2, state: 'running', at: 2 });
    assert.deepStrictEqual((await events.next()).value, { sequence: 3, state: 'completed', at: 2 });
    assert.strictEqual((await events.next()).done, true);
    assert.strictEqual((await machine.events().next()).done, true, 'events are live-only');
  });

  it('latches one deeply immutable terminal receipt', async () => {
    const machine = new LegacyWorkerStateMachine();
    machine.setClusterId('cluster-1');
    machine.transition('starting');
    machine.transition('running');
    assert.strictEqual(machine.terminal(completed()), true);
    assert.strictEqual(
      machine.terminal({
        state: 'failed',
        clusterId: 'cluster-1',
        finishedAt: 4,
        outcome: { status: 'error', code: 'crash', reason: 'declared_failure' },
      }),
      false
    );
    const receipt = await machine.result();
    assert.strictEqual(receipt.state, 'completed');
    assert.ok(Object.isFrozen(receipt.result));
    assert.throws(() => {
      receipt.result.summary = 'changed';
    });
  });

  it('freezes nested receipt data even when the receipt is already shallow-frozen', async () => {
    const machine = new LegacyWorkerStateMachine({ clock: () => 1 });
    machine.setClusterId('cluster-1');
    machine.transition('starting');
    const receipt = Object.freeze({
      state: 'completed',
      clusterId: 'cluster-1',
      finishedAt: 1,
      result: { summary: 'done', status: 'succeeded', artifacts: [] },
    });
    machine.terminal(receipt);
    assert.ok(Object.isFrozen((await machine.result()).result));
  });

  it('rejects cluster mismatches and invalid transitions', () => {
    const machine = new LegacyWorkerStateMachine();
    machine.setClusterId('cluster-1');
    assert.throws(() => machine.transition('running'), /Invalid lifecycle transition/);
    machine.transition('starting');
    assert.throws(() => machine.terminal(completed('cluster-2')), /does not match/);
  });
});
