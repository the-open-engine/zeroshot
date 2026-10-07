import { useEffect, useRef, useState } from 'react';
import type { EnvironmentStore } from './environment-store';

export function useEnvironmentOperation(store: EnvironmentStore, loading: boolean) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const flight = useRef(false);
  const lifetime = useRef(new AbortController());
  useEffect(() => {
    const controller = new AbortController();
    lifetime.current = controller;
    setError('');
    setNotice('');
    setBusy(false);
    flight.current = false;
    return () => controller.abort();
  }, [store]);
  async function perform(action: (signal: AbortSignal) => Promise<void>) {
    if (flight.current || loading) return;
    flight.current = true;
    setBusy(true);
    setError('');
    setNotice('');
    const signal = lifetime.current.signal;
    try {
      await action(signal);
    } catch (cause) {
      if (!signal.aborted) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (!signal.aborted) {
        flight.current = false;
        setBusy(false);
      }
    }
  }
  return { busy, error, notice, setError, setNotice, perform };
}
