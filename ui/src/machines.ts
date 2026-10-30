/** Host-owned resources and lane weights; native profiles retain only the size identity. */
export type MachineOption = {
  size: string;
  label: string;
  vcpus: number;
  memoryMiB: number;
  diskGiB: number;
  laneUnits: number;
  gpu?: { name: string; memoryMiB: number } | null;
};

export function validMachineCatalog(value: unknown): value is MachineOption[] | undefined {
  if (value === undefined) return true;
  if (!Array.isArray(value) || value.length > 32 || !value.every(validMachine)) return false;
  return new Set(value.map((machine) => machine.size)).size === value.length;
}

function validMachine(value: unknown): value is MachineOption {
  const machine = value as Partial<MachineOption> | null;
  if (!machine || !validSize(machine.size) || !validLabel(machine.label)) return false;
  return (
    [machine.vcpus, machine.memoryMiB, machine.diskGiB, machine.laneUnits].every(positiveInteger) &&
    validGpu(machine.gpu)
  );
}

function validSize(value: unknown): value is string {
  return typeof value === 'string' && /^[a-z][a-z0-9_]{0,63}$/.test(value);
}

function validLabel(value: unknown): value is string {
  return (
    typeof value === 'string' &&
    value.trim() === value &&
    value.length > 0 &&
    value.length <= 128 &&
    !/[\u0000-\u001f\u007f]/u.test(value)
  );
}

function positiveInteger(value: unknown): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value > 0;
}

function validGpu(value: unknown): boolean {
  if (value === undefined || value === null) return true;
  const gpu = value as Partial<NonNullable<MachineOption['gpu']>>;
  return validLabel(gpu.name) && positiveInteger(gpu.memoryMiB);
}
