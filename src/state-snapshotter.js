const crypto = require('crypto');
const {
  initStateFromIssue,
  applyIssueOpened,
  applyPlanReady,
  applyWorkerProgress,
  applyImplementationReady,
  applyValidationResult,
  applyInvestigationComplete,
  renderStateSummary,
} = require('./state-snapshot');

const SNAPSHOT_TOPICS = [
  'ISSUE_OPENED',
  'PLAN_READY',
  'WORKER_PROGRESS',
  'IMPLEMENTATION_READY',
  'VALIDATION_RESULT',
  'INVESTIGATION_COMPLETE',
];

class StateSnapshotter {
  constructor({ messageBus, clusterId }) {
    this.messageBus = messageBus;
    this.clusterId = clusterId;
    this.state = null;
    this.lastHash = null;
    this.unsubscribe = null;
  }

  start() {
    if (this.unsubscribe) {
      return;
    }

    this._bootstrapFromLedger();

    this.unsubscribe = this.messageBus.subscribeTopics(SNAPSHOT_TOPICS, (message) => {
      if (message.cluster_id !== this.clusterId) return;
      this._handleMessage(message);
    });
  }

  stop() {
    if (!this.unsubscribe) return;
    this.unsubscribe();
    this.unsubscribe = null;
  }

  _bootstrapFromLedger() {
    const existing = this.messageBus.findLast({
      cluster_id: this.clusterId,
      topic: 'STATE_SNAPSHOT',
    });

    if (existing?.content?.data && typeof existing.content.data === 'object') {
      this.state = existing.content.data;
      this.lastHash = this._hashState(this.state);
      return;
    }

    const messages = SNAPSHOT_TOPICS.map((topic) =>
      this.messageBus.findLast({ cluster_id: this.clusterId, topic })
    ).filter(Boolean);

    if (messages.length === 0) {
      return;
    }

    messages.sort((a, b) => (a.timestamp || 0) - (b.timestamp || 0));

    let state = null;
    for (const message of messages) {
      state = this._applyMessage(state, message);
    }

    if (state) {
      this.state = state;
      this._publishSnapshot(state);
    }
  }

  _handleMessage(message) {
    const nextState = this._applyMessage(this.state, message);
    if (!nextState) return;

    this.state = nextState;
    this._publishSnapshot(nextState);
  }

  _applyMessage(state, message) {
    switch (message.topic) {
      case 'ISSUE_OPENED':
        return state ? applyIssueOpened(state, message) : initStateFromIssue(message);
      case 'PLAN_READY':
        return applyPlanReady(state, message);
      case 'WORKER_PROGRESS':
        return applyWorkerProgress(state, message);
      case 'IMPLEMENTATION_READY':
        return applyImplementationReady(state, message);
      case 'VALIDATION_RESULT':
        return applyValidationResult(state, message);
      case 'INVESTIGATION_COMPLETE':
        return applyInvestigationComplete(state, message);
      default:
        return state;
    }
  }

  _publishSnapshot(state) {
    const hash = this._hashState(state);
    if (this._hashEquals(hash, this.lastHash)) {
      return;
    }
    this.lastHash = hash;

    this.messageBus.publish({
      cluster_id: this.clusterId,
      topic: 'STATE_SNAPSHOT',
      sender: 'state-snapshotter',
      receiver: 'broadcast',
      content: {
        text: renderStateSummary(state),
        data: state,
      },
    });
  }

  _hashState(state) {
    return crypto.createHash('sha256').update(JSON.stringify(state)).digest('hex');
  }

  _hashEquals(left, right) {
    if (!left || !right) return false;
    const leftBuffer = Buffer.from(left, 'utf8');
    const rightBuffer = Buffer.from(right, 'utf8');
    if (leftBuffer.length !== rightBuffer.length) return false;
    return crypto.timingSafeEqual(leftBuffer, rightBuffer);
  }
}

module.exports = StateSnapshotter;
