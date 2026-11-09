/** Target-owned presentation metadata; the target remains authoritative at admission. */
export type RunSizeOption = { id: string; description: string };
export type RunSizeCatalog = {
  kind: 'openengine.run-sizes/v2';
  default?: string | null;
  sizes: RunSizeOption[];
};

export function validSizeId(value: unknown): value is string {
  return typeof value === 'string' && validText(value) && [...value].length <= 128;
}

export function validRunSizeCatalog(value: unknown): value is RunSizeCatalog | undefined {
  if (value === undefined) return true;
  const catalog = value as Partial<RunSizeCatalog> | null;
  if (
    !catalog ||
    catalog.kind !== 'openengine.run-sizes/v2' ||
    !Array.isArray(catalog.sizes) ||
    catalog.sizes.length > 128 ||
    !catalog.sizes.every(validSizeOption)
  )
    return false;
  const ids = new Set(catalog.sizes.map(({ id }) => id));
  return (
    ids.size === catalog.sizes.length &&
    (ids.size === 0
      ? catalog.default === undefined || catalog.default === null
      : typeof catalog.default === 'string' && ids.has(catalog.default))
  );
}

function validSizeOption(value: unknown): value is RunSizeOption {
  const size = value as Partial<RunSizeOption> | null;
  return (
    !!size &&
    validSizeId(size.id) &&
    typeof size.description === 'string' &&
    size.description.trim().length > 0 &&
    validText(size.description) &&
    new TextEncoder().encode(size.description).length <= 1024
  );
}

function validText(value: string): boolean {
  return value.length > 0 && !/[\u0000-\u001f\u007f-\u009f]/u.test(value);
}
