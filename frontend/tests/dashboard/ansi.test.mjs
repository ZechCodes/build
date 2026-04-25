// ansi.js — SGR parser unit coverage.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { ansiToHtml, createAnsiState } from '../../src/dashboard/util/ansi.js';

const ESC = '\x1b';

test('plain text passes through unchanged (html-escaped)', () => {
  const { html } = ansiToHtml('hello <world>');
  assert.equal(html, 'hello &lt;world&gt;');
});

test('basic fg colour wraps text in a span', () => {
  const { html } = ansiToHtml(`${ESC}[31mred${ESC}[0m tail`);
  assert.match(html, /<span class="ansi-fg-1">red<\/span>/);
  assert.match(html, /tail/);
});

test('reset closes the span, subsequent text is plain', () => {
  const { html } = ansiToHtml(`${ESC}[31mred${ESC}[0mplain`);
  assert.match(html, /<span class="ansi-fg-1">red<\/span>plain$/);
});

test('bright colours map to 8–15', () => {
  const { html } = ansiToHtml(`${ESC}[91mbright${ESC}[0m`);
  assert.match(html, /ansi-fg-9/);
});

test('bold + fg compose', () => {
  const { html } = ansiToHtml(`${ESC}[1;33mBOLD${ESC}[0m`);
  assert.match(html, /ansi-fg-3/);
  assert.match(html, /ansi-bold/);
});

test('background colours use ansi-bg-N', () => {
  const { html } = ansiToHtml(`${ESC}[44mblue-bg${ESC}[0m`);
  assert.match(html, /<span class="ansi-bg-4">blue-bg<\/span>/);
});

test('truecolour emits inline style', () => {
  const { html } = ansiToHtml(`${ESC}[38;2;10;20;30mtc${ESC}[0m`);
  assert.match(html, /style="color:rgb\(10,20,30\)"/);
});

test('256-colour maps to xterm palette', () => {
  // 196 is bright red in the xterm 256 cube (step 5,0,0 → 255,0,0).
  const { html } = ansiToHtml(`${ESC}[38;5;196mred256${ESC}[0m`);
  assert.match(html, /style="color:rgb\(255,0,0\)"/);
});

test('unknown CSI final (cursor move) is stripped silently', () => {
  const { html } = ansiToHtml(`A${ESC}[2JB`);
  assert.equal(html, 'AB');
});

test('OSC (terminal title) is stripped', () => {
  const { html } = ansiToHtml(`A${ESC}]0;title${'\x07'}B`);
  assert.equal(html, 'AB');
});

test('state carries across chunks (split escape at boundary)', () => {
  const s0 = createAnsiState();
  const { html: h1, state: s1 } = ansiToHtml(`${ESC}`, s0);
  const { html: h2, state: s2 } = ansiToHtml(`[31mred${ESC}[0m tail`, s1);
  assert.equal(h1, '');
  assert.match(h2, /<span class="ansi-fg-1">red<\/span>/);
  // final state has no pending and default attrs
  assert.equal(s2.pending, '');
});

test('state carries across chunks (split mid-CSI)', () => {
  const s0 = createAnsiState();
  const { html: h1, state: s1 } = ansiToHtml(`${ESC}[3`, s0);
  const { html: h2 }            = ansiToHtml(`1mred${ESC}[0m`, s1);
  assert.equal(h1, '');
  assert.match(h2, /<span class="ansi-fg-1">red<\/span>/);
});

test('attrs persist across chunks when no reset yet', () => {
  const s0 = createAnsiState();
  const { state: s1, html: h1 } = ansiToHtml(`${ESC}[32mA`, s0);
  const { html: h2 }            = ansiToHtml(`B${ESC}[0m`, s1);
  assert.match(h1, /<span class="ansi-fg-2">A<\/span>/);
  assert.match(h2, /<span class="ansi-fg-2">B<\/span>/);
});

test('escape-only input (no content) returns empty html', () => {
  const { html } = ansiToHtml(`${ESC}[31m${ESC}[0m`);
  assert.equal(html, '');
});

test('html inside coloured text is escaped', () => {
  const { html } = ansiToHtml(`${ESC}[31m<script>${ESC}[0m`);
  assert.match(html, /<span class="ansi-fg-1">&lt;script&gt;<\/span>/);
});
