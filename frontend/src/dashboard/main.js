// Expose vendor globals so the dashboard script (and other page scripts) can use them.
import { BuildE2EE } from './vendor/e2ee.js';
import { renderMarkdown, wordDiffLine } from './vendor/markdown.js';

window.BuildE2EE = BuildE2EE;
window.renderMarkdown = renderMarkdown;
window.wordDiffLine = wordDiffLine;

// Dashboard entry — must run after DOM parse (bundle is loaded with `defer`).
import './legacy.js';
