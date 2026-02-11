const fs = require('fs');
const path = require('path');
const os = require('os');
const assert = require('assert');

const { SubagentTracker } = require('../src/subagent-tracker');

const TEST_CLUSTER_ID = 'test-cluster-' + Date.now();
const BASE_DIR = path.join(os.tmpdir(), 'zeroshot-subagents', TEST_CLUSTER_ID);

function writeEvents(agentId, events) {
  const filePath = path.join(BASE_DIR, `${agentId}.jsonl`);
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  const lines = events.map((e) => JSON.stringify(e)).join('\n') + '\n';
  fs.appendFileSync(filePath, lines);
}

afterEach(() => {
  try {
    fs.rmSync(BASE_DIR, { recursive: true, force: true });
  } catch {
    // ignore
  }
});

describe('SubagentTracker', () => {
  it('reads start events and returns active subagents', () => {
    const tracker = new SubagentTracker(TEST_CLUSTER_ID);

    writeEvents('analyst', [
      { event: 'start', agent_id: 'sub-1', description: 'Requirements Analyst review', ts: 1000 },
      { event: 'start', agent_id: 'sub-2', description: 'Logic Flow Tracer review', ts: 2000 },
    ]);

    tracker.poll();
    const active = tracker.getActiveSubagents('analyst');

    assert.strictEqual(active.length, 2);
    assert.strictEqual(active[0].id, 'sub-1');
    assert.strictEqual(active[0].description, 'Requirements Analyst review');
    assert.strictEqual(active[1].id, 'sub-2');
    assert.strictEqual(active[1].description, 'Logic Flow Tracer review');
  });

  it('returns only active subagents (started, not stopped)', () => {
    const tracker = new SubagentTracker(TEST_CLUSTER_ID);

    writeEvents('analyst', [
      { event: 'start', agent_id: 'sub-1', description: 'Requirements Analyst', ts: 1000 },
      { event: 'start', agent_id: 'sub-2', description: 'Logic Flow Tracer', ts: 2000 },
      { event: 'stop', agent_id: 'sub-1', ts: 3000 },
    ]);

    tracker.poll();
    const active = tracker.getActiveSubagents('analyst');

    assert.strictEqual(active.length, 1);
    assert.strictEqual(active[0].id, 'sub-2');
  });

  it('handles missing directory gracefully', () => {
    const tracker = new SubagentTracker('nonexistent-cluster-xyz');

    // Should not throw
    tracker.poll();
    const active = tracker.getActiveSubagents('analyst');
    assert.deepStrictEqual(active, []);
  });

  it('handles malformed JSONL lines', () => {
    const tracker = new SubagentTracker(TEST_CLUSTER_ID);

    const filePath = path.join(BASE_DIR, 'analyst.jsonl');
    fs.mkdirSync(path.dirname(filePath), { recursive: true });
    fs.writeFileSync(
      filePath,
      '{"event":"start","agent_id":"sub-1","description":"Good","ts":1000}\n' +
        'NOT VALID JSON\n' +
        '{"event":"start","agent_id":"sub-2","description":"Also good","ts":2000}\n'
    );

    tracker.poll();
    const active = tracker.getActiveSubagents('analyst');

    // Should have parsed the valid lines, skipped the bad one
    assert.strictEqual(active.length, 2);
    assert.strictEqual(active[0].id, 'sub-1');
    assert.strictEqual(active[1].id, 'sub-2');
  });

  it('offset tracking works on re-poll (no duplicate events)', () => {
    const tracker = new SubagentTracker(TEST_CLUSTER_ID);

    writeEvents('analyst', [{ event: 'start', agent_id: 'sub-1', description: 'First', ts: 1000 }]);

    tracker.poll();
    assert.strictEqual(tracker.getActiveSubagents('analyst').length, 1);

    // Append more events
    writeEvents('analyst', [
      { event: 'start', agent_id: 'sub-2', description: 'Second', ts: 2000 },
    ]);

    tracker.poll();
    const active = tracker.getActiveSubagents('analyst');

    // Should have both, not a duplicate of sub-1
    assert.strictEqual(active.length, 2);
    assert.strictEqual(active[0].id, 'sub-1');
    assert.strictEqual(active[1].id, 'sub-2');
  });

  it('cleanup() removes directory', () => {
    const tracker = new SubagentTracker(TEST_CLUSTER_ID);

    writeEvents('analyst', [{ event: 'start', agent_id: 'sub-1', description: 'Test', ts: 1000 }]);

    assert.ok(fs.existsSync(BASE_DIR));

    tracker.cleanup();

    assert.ok(!fs.existsSync(BASE_DIR));
  });

  it('tracks subagents per parent agent independently', () => {
    const tracker = new SubagentTracker(TEST_CLUSTER_ID);

    writeEvents('analyst', [
      { event: 'start', agent_id: 'sub-1', description: 'Analyst sub', ts: 1000 },
    ]);
    writeEvents('validator', [
      { event: 'start', agent_id: 'sub-2', description: 'Validator sub', ts: 1000 },
    ]);

    tracker.poll();

    assert.strictEqual(tracker.getActiveSubagents('analyst').length, 1);
    assert.strictEqual(tracker.getActiveSubagents('analyst')[0].description, 'Analyst sub');
    assert.strictEqual(tracker.getActiveSubagents('validator').length, 1);
    assert.strictEqual(tracker.getActiveSubagents('validator')[0].description, 'Validator sub');
  });

  it('returns empty array for agent with no subagents', () => {
    const tracker = new SubagentTracker(TEST_CLUSTER_ID);
    tracker.poll();
    assert.deepStrictEqual(tracker.getActiveSubagents('unknown-agent'), []);
  });
});
