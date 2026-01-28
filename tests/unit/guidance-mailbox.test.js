const assert = require('assert');

const Ledger = require('../../src/ledger');
const { USER_GUIDANCE_AGENT, USER_GUIDANCE_CLUSTER } = require('../../src/guidance-topics');

describe('Guidance mailbox', function () {
  it('persists guidance messages with target_agent_id mapped to receiver', function () {
    const ledger = new Ledger(':memory:');
    const clusterId = 'guidance-mailbox-1';

    const published = ledger.append({
      cluster_id: clusterId,
      topic: USER_GUIDANCE_AGENT,
      sender: 'user',
      target_agent_id: 'agent-1',
      content: { text: 'Use approach A' },
    });

    assert.strictEqual(published.receiver, 'agent-1');

    const rows = ledger.query({ cluster_id: clusterId, topic: USER_GUIDANCE_AGENT });
    assert.strictEqual(rows.length, 1);
    assert.strictEqual(rows[0].receiver, 'agent-1');

    ledger.close();
  });

  it('returns cluster + agent guidance since last delivered in deterministic order', function () {
    const ledger = new Ledger(':memory:');
    const clusterId = 'guidance-mailbox-2';
    const now = Date.now();

    ledger.append({
      cluster_id: clusterId,
      topic: USER_GUIDANCE_CLUSTER,
      sender: 'user',
      content: { text: 'Old cluster guidance' },
      timestamp: now + 10,
    });

    ledger.append({
      cluster_id: clusterId,
      topic: USER_GUIDANCE_AGENT,
      sender: 'user',
      target_agent_id: 'agent-a',
      content: { text: 'Old agent guidance' },
      timestamp: now + 20,
    });

    ledger.append({
      cluster_id: clusterId,
      topic: USER_GUIDANCE_CLUSTER,
      sender: 'user',
      content: { text: 'Cluster guidance 1' },
      timestamp: now + 200,
    });

    ledger.append({
      cluster_id: clusterId,
      topic: USER_GUIDANCE_AGENT,
      sender: 'user',
      target_agent_id: 'agent-a',
      content: { text: 'Agent guidance' },
      timestamp: now + 210,
    });

    ledger.append({
      cluster_id: clusterId,
      topic: USER_GUIDANCE_CLUSTER,
      sender: 'user',
      content: { text: 'Cluster guidance 2' },
      timestamp: now + 220,
    });

    const mailbox = ledger.queryGuidanceMailbox({
      cluster_id: clusterId,
      target_agent_id: 'agent-a',
      lastDeliveredAt: now + 100,
    });

    const topics = mailbox.map((message) => message.topic);
    assert.deepStrictEqual(topics, [
      USER_GUIDANCE_CLUSTER,
      USER_GUIDANCE_AGENT,
      USER_GUIDANCE_CLUSTER,
    ]);

    const timestamps = mailbox.map((message) => message.timestamp);
    const sorted = [...timestamps].sort((a, b) => a - b);
    assert.deepStrictEqual(timestamps, sorted);

    const texts = mailbox.map((message) => message.content?.text);
    assert.deepStrictEqual(texts, ['Cluster guidance 1', 'Agent guidance', 'Cluster guidance 2']);

    ledger.close();
  });

  it('excludes guidance for other agents', function () {
    const ledger = new Ledger(':memory:');
    const clusterId = 'guidance-mailbox-3';
    const now = Date.now();

    ledger.append({
      cluster_id: clusterId,
      topic: USER_GUIDANCE_AGENT,
      sender: 'user',
      target_agent_id: 'agent-a',
      content: { text: 'Target agent' },
      timestamp: now + 10,
    });

    ledger.append({
      cluster_id: clusterId,
      topic: USER_GUIDANCE_AGENT,
      sender: 'user',
      target_agent_id: 'agent-b',
      content: { text: 'Other agent' },
      timestamp: now + 20,
    });

    const mailbox = ledger.queryGuidanceMailbox({
      cluster_id: clusterId,
      target_agent_id: 'agent-a',
      lastDeliveredAt: now - 1,
    });

    assert.strictEqual(mailbox.length, 1);
    assert.strictEqual(mailbox[0].content.text, 'Target agent');

    ledger.close();
  });
});
