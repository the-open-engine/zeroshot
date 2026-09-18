import type { GraphNode } from './domain';

export function reviewSources(loop: GraphNode): string[] {
  const result = new Set<string>();
  function visit(value: any, depth: number) {
    if (!value || typeof value !== 'object' || depth > 32) return;
    if (value.value?.source === 'signal' && typeof value.value.name === 'string')
      result.add(value.value.name);
    for (const selector of Array.isArray(value.values) ? value.values : [])
      if (selector?.source === 'signal' && typeof selector.name === 'string')
        result.add(selector.name);
    if (Array.isArray(value.guards)) value.guards.forEach((guard: any) => visit(guard, depth + 1));
    if (value.guard) visit(value.guard, depth + 1);
  }
  visit(loop.until, 0);
  return [...result];
}
