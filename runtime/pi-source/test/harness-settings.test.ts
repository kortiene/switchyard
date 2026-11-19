import assert from 'node:assert/strict';
import test from 'node:test';
import { harnessSettings } from '../src/runtime.ts';

test('an endpoint harness disables model retries', () => {
  assert.deepEqual(harnessSettings(true), {
    settings: {
      retry: { enabled: false, maxRetries: 0 },
      stream: { maxRetries: 0 },
    },
  });
});

test('a no-endpoint harness keeps pi-durable defaults', () => {
  assert.deepEqual(harnessSettings(false), {});
});
