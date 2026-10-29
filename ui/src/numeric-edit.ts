export type NumericDraft = { text: string; badInput: boolean };
export type NumericValue = { valid: true; value: number | undefined } | { valid: false };

/** Native number inputs expose unfinished text such as `1e` as an empty value with badInput. */
export function numericValue(
  draft: NumericDraft,
  optional = false,
  kind: 'positiveInteger' | 'probability' = 'positiveInteger'
): NumericValue {
  if (draft.badInput) return { valid: false };
  if (draft.text === '') return optional ? { valid: true, value: undefined } : { valid: false };
  if (!/^[+-]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?$/.test(draft.text))
    return { valid: false };
  const value = Number(draft.text);
  const valid =
    kind === 'probability'
      ? Number.isFinite(value) && value >= 0 && value <= 1
      : Number.isSafeInteger(value) && value >= 1;
  return valid ? { valid: true, value } : { valid: false };
}
