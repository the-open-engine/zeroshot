import assert from 'node:assert/strict';
import test from 'node:test';
import { atTranscriptBottom, transcriptRange, type TranscriptWindow } from './transcript-window';

const logs = Array.from({ length: 200 }, (_, index) => `Entry ${index}`);
const visible = (total: number, window: TranscriptWindow, follow = true) => {
  const { start, end } = transcriptRange(total, window, follow);
  return logs.slice(start, end);
};

test('reading older output pins entries while live output crosses the render window', () => {
  const window = { start: 0, count: 60, atBottom: false };
  assert.deepEqual(visible(59, window), logs.slice(0, 59));
  assert.deepEqual(visible(60, window), logs.slice(0, 60));
  assert.deepEqual(visible(61, window), logs.slice(0, 60));
  assert.deepEqual(visible(200, window), logs.slice(0, 60));
  assert.deepEqual(visible(200, { ...window, atBottom: true }), logs.slice(140));
});

test('a tail window stays at its recorded position when the reader scrolls away', () => {
  const window = { start: 0, count: 60, atBottom: true };
  const tail = transcriptRange(100, window, true);
  const reading = { ...window, start: tail.start, atBottom: false };
  assert.deepEqual(visible(200, reading), logs.slice(40, 100));
  assert.deepEqual(visible(200, { ...reading, atBottom: true }), logs.slice(140));
});

test('explicit pagination reveals new output without shifting existing entries', () => {
  const reading = { start: 40, count: 60, atBottom: false };
  assert.deepEqual(visible(110, { ...reading, count: 120 }), logs.slice(40, 110));
  assert.deepEqual(visible(150, { ...reading, count: 120 }), logs.slice(40, 150));
});

test('a backward seek never retains future output and clamps windows beyond the prefix', () => {
  const reading = { start: 100, count: 60, atBottom: false };
  assert.deepEqual(visible(130, reading), logs.slice(100, 130));
  assert.deepEqual(visible(80, reading), logs.slice(20, 80));
  assert.deepEqual(visible(0, reading), []);
  const clamped = { ...reading, start: transcriptRange(0, reading, true).start };
  assert.deepEqual(visible(10, clamped), logs.slice(0, 10));
});

test('a paused global cursor does not move the transcript to the tail', () => {
  const window = { start: 40, count: 60, atBottom: true };
  assert.deepEqual(visible(160, window, false), logs.slice(40, 100));
  assert.deepEqual(visible(160, window, true), logs.slice(100, 160));
});

test('sticky scrolling resumes only within the bottom tolerance', () => {
  assert.equal(
    atTranscriptBottom({ scrollHeight: 1000, scrollTop: 500, clientHeight: 300 }),
    false
  );
  assert.equal(
    atTranscriptBottom({ scrollHeight: 1000, scrollTop: 675, clientHeight: 300 }),
    false
  );
  assert.equal(atTranscriptBottom({ scrollHeight: 1000, scrollTop: 676, clientHeight: 300 }), true);
  assert.equal(atTranscriptBottom({ scrollHeight: 200, scrollTop: 0, clientHeight: 300 }), true);
});
