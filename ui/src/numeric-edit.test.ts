import test from 'node:test';
import assert from 'node:assert/strict';
import { numericValue } from './numeric-edit';

test('an unfinished exponent cannot be mistaken for removal of an optional timeout', () => {
  // Browsers return the same empty value for "1e" and a cleared number field.
  assert.deepEqual(numericValue({ text: '', badInput: true }, true), { valid: false });
  assert.deepEqual(numericValue({ text: '', badInput: false }, true), {
    valid: true,
    value: undefined,
  });
  assert.deepEqual(numericValue({ text: '1e3', badInput: false }, true), {
    valid: true,
    value: 1000,
  });
});

test('required numeric drafts preserve emptiness, fractional values, and unsafe integers for repair', () => {
  for (const text of [
    '',
    '1e',
    '-',
    '0',
    '-1',
    '1.5',
    'Infinity',
    '9007199254740992',
    '0x10',
    ' 1 ',
  ]) {
    assert.deepEqual(numericValue({ text, badInput: false }), { valid: false }, text);
  }
  assert.deepEqual(numericValue({ text: '1', badInput: false }), { valid: true, value: 1 });
  assert.deepEqual(numericValue({ text: '9007199254740991', badInput: false }), {
    valid: true,
    value: Number.MAX_SAFE_INTEGER,
  });
});

test('probability drafts accept inclusive endpoints and fractions, rejecting incomplete or invalid text', () => {
  for (const text of ['', '1e', '-0.1', '1.1', 'Infinity', 'NaN', '0x1', ' 0.5 '])
    assert.deepEqual(
      numericValue({ text, badInput: false }, false, 'probability'),
      { valid: false },
      text
    );
  for (const text of ['0', '1', '.85', '1e-2'])
    assert.deepEqual(
      numericValue({ text, badInput: false }, false, 'probability'),
      { valid: true, value: Number(text) },
      text
    );
});
