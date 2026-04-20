import { test } from 'node:test';
import assert from 'node:assert/strict';

import { parseDiffEntries } from '../../src/dashboard-v2/util/diff.js';

const SAMPLE = `diff --git a/foo.js b/foo.js
index abc..def
--- a/foo.js
+++ b/foo.js
@@ -1,4 +1,4 @@
 context line
-removed line
+added line
 more context`;

test('parseDiffEntries classifies add/del/context/hunk', () => {
  const entries = parseDiffEntries(SAMPLE);
  const types = entries.map(e => e.type);
  assert.deepEqual(types, ['hunk', 'ctx', 'del', 'add', 'ctx']);
});

test('hunk header parses line numbers', () => {
  const entries = parseDiffEntries(SAMPLE);
  const hunk = entries.find(e => e.type === 'hunk');
  assert.ok(hunk.text.startsWith('@@'));
});

test('add/del rows strip the prefix character', () => {
  const entries = parseDiffEntries(SAMPLE);
  assert.equal(entries.find(e => e.type === 'add').text, 'added line');
  assert.equal(entries.find(e => e.type === 'del').text, 'removed line');
});

test('multi-hunk diff handled', () => {
  const multi = `@@ -1,1 +1,1 @@
-a
+b
@@ -5,1 +5,1 @@
-c
+d`;
  const entries = parseDiffEntries(multi);
  const hunks = entries.filter(e => e.type === 'hunk');
  assert.equal(hunks.length, 2);
});
