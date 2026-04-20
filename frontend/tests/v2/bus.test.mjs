import { test } from 'node:test';
import assert from 'node:assert/strict';

import { bus } from '../../src/dashboard-v2/core/bus.js';

test('emit with no listeners is a no-op', () => {
  assert.doesNotThrow(() => bus.emit('never.happens', { x: 1 }));
});

test('on/emit delivers payload to a listener', () => {
  const received = [];
  const off = bus.on('t.one', (p) => received.push(p));
  bus.emit('t.one', { a: 1 });
  bus.emit('t.one', { a: 2 });
  off();
  assert.deepEqual(received, [{ a: 1 }, { a: 2 }]);
});

test('multiple listeners all fire, in insertion order', () => {
  const order = [];
  const off1 = bus.on('t.many', () => order.push('a'));
  const off2 = bus.on('t.many', () => order.push('b'));
  const off3 = bus.on('t.many', () => order.push('c'));
  bus.emit('t.many', {});
  off1(); off2(); off3();
  assert.deepEqual(order, ['a', 'b', 'c']);
});

test('unsubscribe stops delivery', () => {
  let count = 0;
  const off = bus.on('t.unsub', () => count++);
  bus.emit('t.unsub', {});
  off();
  bus.emit('t.unsub', {});
  assert.equal(count, 1);
});

test('a throwing listener does not block others', () => {
  const saw = [];
  const origError = console.error;
  console.error = () => {};          // silence the [bus] error log for the test
  const off1 = bus.on('t.throw', () => { throw new Error('boom'); });
  const off2 = bus.on('t.throw', () => saw.push('after'));
  bus.emit('t.throw', {});
  console.error = origError;
  off1(); off2();
  assert.deepEqual(saw, ['after']);
});

test('emit to an unknown type does not error', () => {
  assert.doesNotThrow(() => bus.emit('totally.unknown', { any: 'thing' }));
});
