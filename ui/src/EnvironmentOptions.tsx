import type { EnvironmentSummary } from './environment-store';

export function EnvironmentOptions({ items }: { items: EnvironmentSummary[] }) {
  return items.map(({ id, name }) => (
    <option key={id} value={id}>
      {name}
    </option>
  ));
}
