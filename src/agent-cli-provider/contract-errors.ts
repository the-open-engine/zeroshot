export class ContractRequestError extends Error {
  readonly code: string;
  readonly exitCode: number;
  readonly field?: string;

  constructor(input: { code: string; message: string; exitCode: number; field?: string }) {
    super(input.message);
    this.name = 'ContractRequestError';
    this.code = input.code;
    this.exitCode = input.exitCode;
    if (input.field !== undefined) this.field = input.field;
  }
}

export function contractError(input: {
  readonly code: string;
  readonly message: string;
  readonly exitCode: number;
  readonly field?: string;
}): ContractRequestError {
  return new ContractRequestError(input);
}

export function invalidField(field: string, message: string): never {
  throw contractError({
    code: 'invalid-field',
    message,
    exitCode: 2,
    field,
  });
}

export function requiredString(record: Record<string, unknown>, field: string): string {
  const value = record[field];
  if (typeof value === 'string' && value.length > 0) return value;
  throw contractError({
    code: 'missing-field',
    message: `${field} is required.`,
    exitCode: 2,
    field,
  });
}

export function optionalString(record: Record<string, unknown>, field: string): string | undefined {
  const value = record[field];
  if (value === undefined) return undefined;
  if (typeof value === 'string') return value;
  throw contractError({
    code: 'invalid-field',
    message: `${field} must be a string.`,
    exitCode: 2,
    field,
  });
}

export function optionalNumber(record: Record<string, unknown>, field: string): number | undefined {
  const value = record[field];
  if (value === undefined) return undefined;
  if (typeof value === 'number' && Number.isFinite(value)) return value;
  throw contractError({
    code: 'invalid-field',
    message: `${field} must be a finite number.`,
    exitCode: 2,
    field,
  });
}
