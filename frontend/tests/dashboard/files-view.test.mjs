import { test } from 'node:test';
import assert from 'node:assert/strict';

import { isHtmlPreviewable } from '../../src/dashboard/channel/views/files-view.js';

test('HTML preview is only enabled for html files', () => {
  assert.equal(isHtmlPreviewable('index.html'), true);
  assert.equal(isHtmlPreviewable('pages/settings.HTM'), true);
  assert.equal(isHtmlPreviewable('styles/app.css'), false);
  assert.equal(isHtmlPreviewable('README.md'), false);
  assert.equal(isHtmlPreviewable('html'), false);
});
