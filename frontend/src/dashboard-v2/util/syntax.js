// Code syntax highlighting. Wave 4 stub — returns plain escaped text.
// Wave 5 will port the real highlighter from v1 files/syntax.js.

import { escapeHtml } from './html.js';

export function highlightLine(line, _langExt) {
  return escapeHtml(line);
}
