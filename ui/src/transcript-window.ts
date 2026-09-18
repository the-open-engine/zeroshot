export const TRANSCRIPT_PAGE_SIZE = 60;

export interface TranscriptWindow {
  start: number;
  count: number;
  atBottom: boolean;
}

/** Only the tail moves on incoming output. A reader's window stays on the same entries. */
export function transcriptRange(total: number, window: TranscriptWindow, follow: boolean) {
  const tail = Math.max(0, total - window.count);
  const start = follow && window.atBottom ? tail : window.start < total ? window.start : tail;
  return { start, end: Math.min(total, start + window.count) };
}

export function atTranscriptBottom(box: {
  scrollHeight: number;
  scrollTop: number;
  clientHeight: number;
}) {
  return box.scrollHeight - box.scrollTop - box.clientHeight <= 24;
}
