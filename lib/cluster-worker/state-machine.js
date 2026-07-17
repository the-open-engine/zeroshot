'use strict';

const {
  TERMINAL_STATES,
  validateLifecycleStatus,
  validateTerminalReceipt,
} = require('./contracts');
const { deepFreeze } = require('./object-utils');

const ALLOWED = Object.freeze({
  idle: new Set(['starting']),
  starting: new Set([
    'running',
    'stopping',
    'completed',
    'failed',
    'timed_out',
    'stopped',
    'malformed',
  ]),
  running: new Set(['stopping', 'completed', 'failed', 'timed_out', 'stopped', 'malformed']),
  stopping: new Set(['stopped', 'completed', 'failed', 'timed_out', 'malformed']),
});

class LiveEventIterator {
  constructor(remove) {
    this.remove = remove;
    this.queue = [];
    this.waiters = [];
    this.closed = false;
  }

  push(value) {
    if (this.closed) return;
    const waiter = this.waiters.shift();
    if (waiter) waiter({ value, done: false });
    else this.queue.push(value);
  }

  next() {
    if (this.queue.length > 0) return Promise.resolve({ value: this.queue.shift(), done: false });
    if (this.closed) return Promise.resolve({ value: undefined, done: true });
    return new Promise((resolve) => this.waiters.push(resolve));
  }

  return() {
    if (!this.closed) {
      this.closed = true;
      this.remove();
      for (const waiter of this.waiters.splice(0)) waiter({ value: undefined, done: true });
    }
    return Promise.resolve({ value: undefined, done: true });
  }

  close() {
    if (this.closed) return;
    this.closed = true;
    this.remove();
    for (const waiter of this.waiters.splice(0)) waiter({ value: undefined, done: true });
  }

  [Symbol.asyncIterator]() {
    return this;
  }
}

class LegacyWorkerStateMachine {
  constructor({ clock = () => Date.now() } = {}) {
    this.clock = clock;
    this.state = 'idle';
    this.clusterId = null;
    this.sequence = 0;
    this.stopRequested = false;
    this.terminalReceipt = null;
    this.subscribers = new Set();
    this.resultPromise = new Promise((resolve) => {
      this.resolveResult = resolve;
    });
  }

  setClusterId(clusterId) {
    if (this.clusterId && this.clusterId !== clusterId) {
      throw new Error('Worker already owns a different cluster resource');
    }
    this.clusterId = clusterId;
  }

  status() {
    return validateLifecycleStatus({
      state: this.state,
      clusterId: this.clusterId,
      sequence: this.sequence,
      stopRequested: this.stopRequested,
      terminal: TERMINAL_STATES.includes(this.state),
    });
  }

  events() {
    let iterator;
    iterator = new LiveEventIterator(() => this.subscribers.delete(iterator));
    this.subscribers.add(iterator);
    if (this.terminalReceipt) iterator.close();
    return iterator;
  }

  transition(next, details) {
    if (TERMINAL_STATES.includes(this.state)) return false;
    const allowed = ALLOWED[this.state];
    if (!allowed?.has(next))
      throw new Error(`Invalid lifecycle transition ${this.state} -> ${next}`);
    this.state = next;
    this.sequence += 1;
    const event = deepFreeze({
      sequence: this.sequence,
      state: next,
      at: this.clock(),
      ...(details === undefined ? {} : { details }),
    });
    for (const subscriber of this.subscribers) subscriber.push(event);
    return true;
  }

  requestStop() {
    this.stopRequested = true;
    if (this.state === 'starting' || this.state === 'running') this.transition('stopping');
  }

  terminal(receipt) {
    if (this.terminalReceipt) return false;
    validateTerminalReceipt(receipt);
    if (receipt.clusterId !== this.clusterId) {
      throw new Error('Terminal receipt clusterId does not match owned resource');
    }
    const frozenReceipt = deepFreeze(receipt);
    this.transition(receipt.state);
    this.terminalReceipt = frozenReceipt;
    this.resolveResult(this.terminalReceipt);
    for (const subscriber of [...this.subscribers]) subscriber.close();
    return true;
  }

  result() {
    return this.resultPromise;
  }
}

module.exports = { LegacyWorkerStateMachine };
