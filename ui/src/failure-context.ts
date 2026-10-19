export type FailureDiagnostic = {
  message: string;
  execution?: { execution: string; node: string } | null;
  cursor?: string | null;
};
export type FailureContext = {
  primary?: FailureDiagnostic | null;
  recorded?: FailureDiagnostic[];
  incomplete?: boolean;
};

const encoder = new TextEncoder();
const controls = /[\u0000-\u001f\u007f-\u009f]/;
function boundedText(value: unknown, maximum: number): value is string {
  return (
    typeof value === 'string' && !controls.test(value) && encoder.encode(value).length <= maximum
  );
}
function validDiagnostic(value: unknown): value is FailureDiagnostic {
  const diagnostic = value as Partial<FailureDiagnostic> | null;
  if (!diagnostic || !boundedText(diagnostic.message, 16_384)) return false;
  if (diagnostic.cursor != null && typeof diagnostic.cursor !== 'string') return false;
  const execution = diagnostic.execution;
  return (
    execution == null ||
    (boundedText(execution.execution, 128) &&
      execution.execution.length > 0 &&
      typeof execution.node === 'string' &&
      execution.node.length <= 128 &&
      /^[A-Za-z_][A-Za-z0-9_.-]*$/.test(execution.node))
  );
}

function validRecorded(value: unknown): boolean {
  return (
    value === undefined ||
    (Array.isArray(value) && value.length <= 5 && value.every(validDiagnostic))
  );
}

/** Native public diagnostic bounds also apply to observation metadata without a terminal event. */
export function validFailureContext(value: unknown): value is FailureContext {
  const context = value as Partial<FailureContext> | null;
  return (
    !!context &&
    typeof context === 'object' &&
    !Array.isArray(context) &&
    (context.incomplete === undefined || typeof context.incomplete === 'boolean') &&
    (context.primary == null || validDiagnostic(context.primary)) &&
    validRecorded(context.recorded)
  );
}
