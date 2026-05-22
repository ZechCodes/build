import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  allTreeRefreshDirs,
  isPreviewable,
} from '../../src/dashboard/channel/views/files-view.js';

test('Preview is enabled for HTML, Markdown, SVG, and image files', () => {
  assert.equal(isPreviewable('index.html'), true);
  assert.equal(isPreviewable('pages/settings.HTM'), true);
  assert.equal(isPreviewable('README.md'), true);
  assert.equal(isPreviewable('notes.markdown'), true);
  assert.equal(isPreviewable('logo.SVG'), true);
  assert.equal(isPreviewable('screenshot.png'), true);
  assert.equal(isPreviewable('photo.JPG'), true);
  assert.equal(isPreviewable('avatar.webp'), true);
  assert.equal(isPreviewable('styles/app.css'), false);
  assert.equal(isPreviewable('main.py'), false);
  assert.equal(isPreviewable('html'), false);
});

test('All tree refresh includes root, expanded dirs, and changed file ancestors', () => {
  assert.deepEqual(
    allTreeRefreshDirs(['src/pages/index.html', 'README.md'], ['src', 'assets/icons']),
    ['', 'src', 'assets/icons', 'src/pages'],
  );
});
