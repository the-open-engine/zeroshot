import { useEffect, useState } from 'react';
import type { EnvironmentStore, EnvironmentSummary } from './environment-store';

export function useEnvironmentCatalog(store: EnvironmentStore) {
  const [items, setItems] = useState<EnvironmentSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  useEffect(() => {
    const controller = new AbortController();
    setItems([]);
    setError('');
    setLoading(true);
    void store
      .list(controller.signal)
      .then((result) => {
        if (!controller.signal.aborted) setItems(result.environments);
      })
      .catch((cause) => {
        if (!controller.signal.aborted)
          setError(cause instanceof Error ? cause.message : String(cause));
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false);
      });
    return () => controller.abort();
  }, [store]);
  return { items, setItems, loading, error };
}
