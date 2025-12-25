/**
 * Hello Vibe Test
 *
 * Simple test that echoes "hello from zeroshot" and exits successfully.
 */

import { test, expect } from '@playwright/test';

test('should echo hello from zeroshot', async () => {
  // Echo hello from zeroshot
  console.log('hello from zeroshot');

  // Exit successfully
  expect(true).toBe(true);
});
