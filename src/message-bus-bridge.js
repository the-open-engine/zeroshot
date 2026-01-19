/**
 * MessageBusBridge - Bridges parent and child message buses
 *
 * Forwards specified topics between parent and child clusters while maintaining
 * isolation and preventing message loops.
 *
 * Features:
 * - Forward specified parent topics to child (contextStrategy.parentTopics)
 * - Forward child completion events to parent
 * - Namespace child topics to avoid collisions
 * - Prevent message loops via forwarding flags
 */

class MessageBusBridge {
  constructor(parentBus, childBus, config) {
    this.parentBus = parentBus;
    this.childBus = childBus;
    this.config = config;
    this.parentTopicNames = new Set(
      (config.parentTopics || [])
        .map((entry) => (typeof entry === 'string' ? entry : entry?.topic))
        .filter((topic) => typeof topic === 'string' && topic.length > 0)
    );

    this.parentUnsubscribe = null;
    this.childUnsubscribe = null;
    this.active = false;

    this._setupBridge();
  }

  /**
   * Set up bidirectional message forwarding
   * @private
   */
  _setupBridge() {
    // Forward specified parent topics to child
    if (this.parentTopicNames.size > 0) {
      this.parentUnsubscribe = this.parentBus.subscribe((message) => {
        this._forwardParentToChild(message);
      });
    }

    // Forward child completion/failure events to parent
    this.childUnsubscribe = this.childBus.subscribe((message) => {
      this._forwardChildToParent(message);
    });

    this.active = true;
  }

  /**
   * Forward parent message to child cluster
   * @private
   */
  _forwardParentToChild(message) {
    // Only forward messages from parent cluster
    if (message.cluster_id !== this.config.parentClusterId) {
      return;
    }

    // Only forward topics specified in config
    if (!this.parentTopicNames.has(message.topic)) {
      return;
    }

    // Skip already-forwarded messages (prevent loops)
    if (message.metadata?.forwarded) {
      return;
    }

    // Forward to child with metadata flag
    this.childBus.publish({
      ...message,
      cluster_id: this.config.childClusterId,
      metadata: {
        ...message.metadata,
        forwarded: true,
        forwardedFrom: this.config.parentClusterId,
      },
    });
  }

  /**
   * Forward child message to parent cluster
   * @private
   */
  _forwardChildToParent(message) {
    // Only forward messages from child cluster
    if (message.cluster_id !== this.config.childClusterId) {
      return;
    }

    // Only forward completion/failure events
    const forwardTopics = ['CLUSTER_COMPLETE', 'CLUSTER_FAILED', 'AGENT_ERROR'];
    if (!forwardTopics.includes(message.topic)) {
      return;
    }

    // Skip already-forwarded messages (prevent loops)
    if (message.metadata?.forwarded) {
      return;
    }

    // Forward to parent with namespaced topic and metadata flag
    this.parentBus.publish({
      ...message,
      cluster_id: this.config.parentClusterId,
      topic: `CHILD_${message.topic}`,
      metadata: {
        ...message.metadata,
        forwarded: true,
        forwardedFrom: this.config.childClusterId,
        childClusterId: this.config.childClusterId,
      },
    });
  }

  /**
   * Close the bridge and stop forwarding
   */
  close() {
    if (this.parentUnsubscribe) {
      this.parentUnsubscribe();
      this.parentUnsubscribe = null;
    }

    if (this.childUnsubscribe) {
      this.childUnsubscribe();
      this.childUnsubscribe = null;
    }

    this.active = false;
  }

  /**
   * Check if bridge is active
   */
  isActive() {
    return this.active;
  }
}

module.exports = MessageBusBridge;
