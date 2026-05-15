import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  allTreeRefreshDirs,
  isHtmlPreviewable,
} from '../../src/dashboard/channel/views/files-view.js';

test('HTML preview is only enabled for html files', () => {
  assert.equal(isHtmlPreviewable('index.html'), true);
  assert.equal(isHtmlPreviewable('pages/settings.HTM'), true);
  assert.equal(isHtmlPreviewable('styles/app.css'), false);
  assert.equal(isHtmlPreviewable('README.md'), false);
  assert.equal(isHtmlPreviewable('html'), false);
});

test('All tree refresh includes root, expanded dirs, and changed file ancestors', () => {
  assert.deepEqual(
    allTreeRefreshDirs(['src/pages/index.html', 'README.md'], ['src', 'assets/icons']),
    ['', 'src', 'assets/icons', 'src/pages'],
  );
});
