import { state } from '../state.js';
import { fileContentBody } from './refs.js';
import { highlightLine } from './syntax.js';
import { wordDiffLine } from '../vendor/markdown.js';

export function renderDiffContent(diffText, truncated) {
  fileContentBody.style.padding = '';
  const lines = diffText.split('\n');
  const viewer = document.createElement('div');
  viewer.className = 'diff-viewer';

  // Derive file extension for syntax highlighting.
  const extMatch = state.filesCurrentPath ? state.filesCurrentPath.match(/\.([^./]+)$/) : null;
  const ext = extMatch ? extMatch[1].toLowerCase() : '';

  // Pre-parse lines into typed entries.
  const entries = [];
  let oldNum = 0, newNum = 0;
  for (const line of lines) {
    if (line.startsWith('diff ') || line.startsWith('index ') || line.startsWith('---') || line.startsWith('+++')) continue;
    if (line.startsWith('@@')) {
      const m = line.match(/@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
      if (m) { oldNum = parseInt(m[1]); newNum = parseInt(m[2]); }
      entries.push({ type: 'hunk', text: line });
      continue;
    }
    if (line.startsWith('+')) {
      entries.push({ type: 'add', text: line.slice(1), num: String(newNum++) });
    } else if (line.startsWith('-')) {
      entries.push({ type: 'del', text: line.slice(1), num: String(oldNum++) });
    } else {
      entries.push({ type: 'ctx', text: line.startsWith(' ') ? line.slice(1) : line, num: String(newNum) });
      oldNum++; newNum++;
    }
  }

  function addRow(cls, num, html) {
    const row = document.createElement('div');
    row.className = 'diff-line ' + cls;
    const numEl = document.createElement('span');
    numEl.className = 'dl-num';
    numEl.textContent = num;
    const contentEl = document.createElement('span');
    contentEl.className = 'dl-content';
    contentEl.innerHTML = html;
    row.appendChild(numEl);
    row.appendChild(contentEl);
    viewer.appendChild(row);
  }

  for (let ei = 0; ei < entries.length; ei++) {
    const e = entries[ei];
    if (e.type === 'hunk') {
      const hdr = document.createElement('div');
      hdr.className = 'diff-hunk-header';
      hdr.textContent = e.text;
      viewer.appendChild(hdr);
      continue;
    }
    if (e.type === 'ctx') {
      addRow('context', e.num, highlightLine(e.text, ext));
      continue;
    }
    if (e.type === 'del') {
      const dels = [e];
      while (ei + 1 < entries.length && entries[ei + 1].type === 'del') dels.push(entries[++ei]);
      const adds = [];
      while (ei + 1 < entries.length && entries[ei + 1].type === 'add') adds.push(entries[++ei]);
      const pairs = Math.min(dels.length, adds.length);
      for (let pi = 0; pi < dels.length; pi++) {
        const html = pi < pairs ? wordDiffLine(dels[pi].text, adds[pi].text).oldHtml : highlightLine(dels[pi].text, ext);
        addRow('removed', dels[pi].num, html);
      }
      for (let ai = 0; ai < adds.length; ai++) {
        const html = ai < pairs ? wordDiffLine(dels[ai].text, adds[ai].text).newHtml : highlightLine(adds[ai].text, ext);
        addRow('added', adds[ai].num, html);
      }
      continue;
    }
    if (e.type === 'add') {
      addRow('added', e.num, highlightLine(e.text, ext));
    }
  }

  fileContentBody.innerHTML = '';
  fileContentBody.appendChild(viewer);

  if (truncated) {
    const note = document.createElement('div');
    note.style.cssText = 'padding:.5rem .75rem;font-size:11px;color:var(--text-muted);border-top:1px solid var(--border)';
    note.textContent = 'Diff truncated';
    fileContentBody.appendChild(note);
  }
}
