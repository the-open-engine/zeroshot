import { randomUUID } from 'crypto';

export type SubscriptionKind = string;

export type SubscriptionEntry = {
  id: string;
  kind: SubscriptionKind;
  close: () => void;
  closed: boolean;
};

export type SubscriptionRegistry = {
  add: (kind: SubscriptionKind, close: () => void) => string;
  unsubscribe: (id: string) => { removed: boolean };
  closeAll: () => number;
  size: () => number;
};

export function createSubscriptionRegistry(): SubscriptionRegistry {
  const entries = new Map<string, SubscriptionEntry>();

  const add = (kind: SubscriptionKind, close: () => void) => {
    if (typeof close !== 'function') {
      throw new TypeError('Subscription close must be a function.');
    }
    const id = randomUUID();
    entries.set(id, {
      id,
      kind,
      close,
      closed: false,
    });
    return id;
  };

  const unsubscribe = (id: string) => {
    const entry = entries.get(id);
    if (!entry) {
      return { removed: false };
    }
    entries.delete(id);
    if (!entry.closed) {
      entry.closed = true;
      entry.close();
    }
    return { removed: true };
  };

  const closeAll = () => {
    const values = Array.from(entries.values());
    entries.clear();
    for (const entry of values) {
      if (!entry.closed) {
        entry.closed = true;
        entry.close();
      }
    }
    return values.length;
  };

  const size = () => entries.size;

  return {
    add,
    unsubscribe,
    closeAll,
    size,
  };
}
