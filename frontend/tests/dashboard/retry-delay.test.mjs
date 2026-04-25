// _retryDelayMs — picks the wait before a retry after a 429. Must
// respect Retry-After (seconds or HTTP date), fall back to jittered
// exponential backoff, and never return a negative/zero-wait.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { _retryDelayMs } from '../../src/dashboard/transport/vendor/e2ee.js';

function respWith(header) {
  return { headers: { get: (name) => name === 'Retry-After' ? header : null } };
}

test('Retry-After as seconds', () => {
  assert.equal(_retryDelayMs(respWith('3'), 1), 3000);
});

test('Retry-After as fractional seconds rounds to ms', () => {
  // Number('1.5') = 1.5 → 1500ms, capped at 10s via Math.min
  assert.equal(_retryDelayMs(respWith('1.5'), 1), 1500);
});

test('Retry-After seconds are capped at 10s', () => {
  assert.equal(_retryDelayMs(respWith('60'), 1), 10_000);
});

test('Retry-After as HTTP date in the future', () => {
  // UTCString drops sub-second precision, so the actual delta can
  // float within a ~1s window around the target. Use 5s to make the
  // bound comfortable regardless of scheduler jitter.
  const future = new Date(Date.now() + 5000).toUTCString();
  const delay = _retryDelayMs(respWith(future), 1);
  assert.ok(delay >= 3500 && delay <= 6000, `delay=${delay}`);
});

test('Retry-After HTTP date in the past → falls back to backoff', () => {
  const past = new Date(Date.now() - 5000).toUTCString();
  const delay = _retryDelayMs(respWith(past), 1);
  // First attempt backoff baseline is 200ms; jitter ±25% → 150..250.
  assert.ok(delay >= 150 && delay <= 250, `delay=${delay}`);
});

test('no Retry-After → exponential backoff increases per attempt', () => {
  // Collect the midpoints — jitter is ±25% so average out:
  const base = (attempt) => {
    let sum = 0;
    for (let i = 0; i < 80; i++) sum += _retryDelayMs(respWith(null), attempt);
    return sum / 80;
  };
  const d1 = base(1);   // ~200
  const d2 = base(2);   // ~400
  const d3 = base(3);   // ~800
  assert.ok(d2 > d1 * 1.5, `d2=${d2} d1=${d1}`);
  assert.ok(d3 > d2 * 1.5, `d3=${d3} d2=${d2}`);
});

test('backoff always returns at least 50ms', () => {
  // Even when the random jitter pulls toward the negative end.
  for (let i = 0; i < 50; i++) {
    const d = _retryDelayMs(respWith(null), 1);
    assert.ok(d >= 50, `delay=${d}`);
  }
});

test('unparseable Retry-After → falls back to backoff', () => {
  const d = _retryDelayMs(respWith('tomorrow'), 1);
  assert.ok(d >= 150 && d <= 250, `delay=${d}`);
});
