export function stripTimestampPrefix(line: string): string {
  let trimmed = line.trim().replace(/\r$/, '');
  if (!trimmed) return '';

  const timestampMatch = /^\[(\d{13})\](.*)$/.exec(trimmed);
  const timestampRemainder = timestampMatch?.[2];
  if (typeof timestampRemainder === 'string') {
    trimmed = timestampRemainder.trimStart();
  }

  if (!trimmed.startsWith('{') && !trimmed.startsWith('[')) {
    const pipeMatch = /^[^|]{1,40}\|\s*(.*)$/.exec(trimmed);
    const afterPipe = pipeMatch?.[1]?.trimStart();
    if (typeof afterPipe === 'string' && (afterPipe.startsWith('{') || afterPipe.startsWith('['))) {
      return afterPipe;
    }
  }

  return trimmed;
}
