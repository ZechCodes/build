import { test } from 'node:test';
import assert from 'node:assert/strict';

import { highlightLine } from '../../src/dashboard/util/syntax.js';

test('unknown ext returns escaped plain text', () => {
  const out = highlightLine('const x = 1;', 'xyz');
  assert.ok(out.includes('const x = 1;'));
  assert.ok(!out.includes('tok-'));
});

test('js keyword gets tok-keyword class', () => {
  const out = highlightLine('const x = 1;', 'js');
  assert.match(out, /<span class="tok-keyword">const<\/span>/);
});

test('js string literal gets tok-string class', () => {
  const out = highlightLine('let s = "hello";', 'js');
  assert.match(out, /tok-string[^>]*>&quot;hello&quot;/);
});

test('js comment gets tok-comment class', () => {
  const out = highlightLine('// note this', 'js');
  assert.match(out, /<span class="tok-comment">\/\/ note this<\/span>/);
});

test('html opening tag gets tok-keyword class', () => {
  const out = highlightLine('<div class="a">', 'html');
  assert.match(out, /tok-keyword/);
});

test('python keyword detected', () => {
  const out = highlightLine('def foo():', 'py');
  assert.match(out, /<span class="tok-keyword">def<\/span>/);
});

test('empty line returns non-breaking space', () => {
  const out = highlightLine('', 'js');
  assert.equal(out, '');
});

test('html is escaped even when no highlighter matches', () => {
  const out = highlightLine('<script>alert(1)</script>', 'xyz');
  assert.ok(out.startsWith('&lt;script'));
  assert.ok(!out.includes('<script>'));
});
