// markdown.js — <build-image> rendering branches.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { renderMarkdown } from '../../src/dashboard/util/markdown.js';

test('legacy <build-image> with base64 body renders an inline data-URI', () => {
  // 1x1 transparent PNG, base64-encoded.
  const b64 =
    'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR4nGNgYAAAAAMAASsJTYQAAAAASUVORK5CYII=';
  const md = `<build-image path="/x/y.png" mime="image/png">\n${b64}\n</build-image>`;
  const out = renderMarkdown(md);
  assert.match(out, /<figure class="v2-embed-image"/);
  assert.match(out, /src="data:image\/png;base64,iVBORw0/);
  assert.doesNotMatch(out, /v2-embed-image-lazy/);
});

test('body-less <build-image> renders a lazy placeholder with data attributes', () => {
  const md = '<build-image path="/Users/me/shot.png" mime="image/png"></build-image>';
  const out = renderMarkdown(md);
  assert.match(out, /v2-embed-image-lazy/);
  assert.match(out, /data-build-image-path="\/Users\/me\/shot\.png"/);
  assert.match(out, /data-build-image-mime="image\/png"/);
  // No src= attribute on the lazy img — chat-view sets it when the
  // fetch completes.
  assert.doesNotMatch(out, /<img[^>]*\bsrc=/);
});

test('truncated stripped body falls through to the lazy placeholder', () => {
  // The bridge previously inlined base64; rows we manually truncated
  // contain "[image data stripped]" as the body. That's not valid
  // base64 — the renderer must treat it as missing and lazy-fetch.
  const md = '<build-image path="/x/y.png" mime="image/png">[image data stripped]</build-image>';
  const out = renderMarkdown(md);
  assert.match(out, /v2-embed-image-lazy/);
  assert.doesNotMatch(out, /<img[^>]*\bsrc=/);
});

test('whitespace-only body is treated as empty', () => {
  const md = '<build-image path="/x/y.png" mime="image/png">\n   \n</build-image>';
  const out = renderMarkdown(md);
  assert.match(out, /v2-embed-image-lazy/);
});
