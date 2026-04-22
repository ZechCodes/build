// Minimal ANSI SGR parser. Emits HTML for a scrolling terminal
// scrollback — enough to make `ls --color`, `git`, `grep`, `cargo`
// look right. Cursor-movement, OSC, and other non-SGR escape
// sequences are stripped; we're not driving a full terminal grid.
//
// Usage:
//   const state = createAnsiState();
//   const { html, state: next } = ansiToHtml(chunk1, state);
//   out.innerHTML += html;
//   const { html: more, state: final } = ansiToHtml(chunk2, next);
//
// The state carries:
//   - the current SGR attrs (fg/bg/bold/etc.)
//   - a `pending` buffer holding a trailing partial escape so
//     splitting `\x1b[` across chunks doesn't drop the sequence.

import { escapeHtml } from './html.js';

const DEFAULT_ATTRS = Object.freeze({
  fg: null,        // null | number (0–7, bright 8–15) | '256:N' | 'rgb:R,G,B'
  bg: null,
  bold: false,
  dim: false,
  italic: false,
  underline: false,
  reverse: false,
  strike: false,
});

export function createAnsiState() {
  return { attrs: { ...DEFAULT_ATTRS }, pending: '' };
}

export function ansiToHtml(chunk, state = createAnsiState()) {
  const input = (state.pending || '') + (chunk || '');
  let attrs = { ...state.attrs };
  let pending = '';
  const out = [];

  let i = 0;
  const len = input.length;
  let plain = '';   // buffer of plain text being accumulated under `attrs`
  const flushPlain = () => {
    if (!plain) return;
    out.push(wrapSpan(escapeHtml(plain), attrs));
    plain = '';
  };

  while (i < len) {
    const ch = input[i];
    if (ch !== '\x1b') {
      plain += ch;
      i++;
      continue;
    }
    // ESC — try to consume a known sequence.
    // Bail early if we don't have at least the next byte (partial
    // sequence at the tail of the chunk): buffer it and resume next
    // call.
    if (i + 1 >= len) {
      pending = input.slice(i);
      break;
    }
    const next = input[i + 1];
    if (next === '[') {
      // CSI: ESC [ params? final
      //   params = 0–9 and ; and :
      //   final  = byte in 0x40–0x7e
      let j = i + 2;
      while (j < len) {
        const c = input[j];
        const code = c.charCodeAt(0);
        if (code >= 0x40 && code <= 0x7e) break;
        j++;
      }
      if (j >= len) {
        pending = input.slice(i);
        break;
      }
      const final = input[j];
      const params = input.slice(i + 2, j);
      i = j + 1;
      if (final === 'm') {
        flushPlain();
        attrs = applySgr(attrs, params);
      }
      // Non-'m' finals (cursor move, clear, etc.) are dropped.
      continue;
    }
    if (next === ']') {
      // OSC: ESC ] payload ST  where ST is BEL or ESC \
      let j = i + 2;
      while (j < len) {
        const c = input[j];
        if (c === '\x07') { j++; break; }
        if (c === '\x1b' && j + 1 < len && input[j + 1] === '\\') { j += 2; break; }
        j++;
      }
      if (j >= len) {
        pending = input.slice(i);
        break;
      }
      i = j;
      continue;
    }
    // Two-byte escape like ESC =, ESC (B, ESC 7 — drop the next byte.
    i += 2;
  }

  if (plain) {
    out.push(wrapSpan(escapeHtml(plain), attrs));
  }
  return { html: out.join(''), state: { attrs, pending } };
}

function applySgr(attrs, params) {
  if (!params) return { ...DEFAULT_ATTRS };   // `\x1b[m` ≡ reset
  const parts = params.split(';').map(p => p === '' ? 0 : Number(p));
  let next = { ...attrs };
  for (let i = 0; i < parts.length; i++) {
    const n = parts[i];
    if (!Number.isFinite(n)) continue;
    if (n === 0)  { next = { ...DEFAULT_ATTRS }; continue; }
    if (n === 1)  { next.bold      = true;  continue; }
    if (n === 2)  { next.dim       = true;  continue; }
    if (n === 3)  { next.italic    = true;  continue; }
    if (n === 4)  { next.underline = true;  continue; }
    if (n === 7)  { next.reverse   = true;  continue; }
    if (n === 9)  { next.strike    = true;  continue; }
    if (n === 22) { next.bold = next.dim = false; continue; }
    if (n === 23) { next.italic = false;    continue; }
    if (n === 24) { next.underline = false; continue; }
    if (n === 27) { next.reverse = false;   continue; }
    if (n === 29) { next.strike = false;    continue; }
    if (n >= 30 && n <= 37)   { next.fg = n - 30;       continue; }
    if (n === 39)             { next.fg = null;          continue; }
    if (n >= 40 && n <= 47)   { next.bg = n - 40;       continue; }
    if (n === 49)             { next.bg = null;          continue; }
    if (n >= 90 && n <= 97)   { next.fg = (n - 90) + 8; continue; }  // bright
    if (n >= 100 && n <= 107) { next.bg = (n - 100) + 8; continue; }
    if (n === 38 || n === 48) {
      const target = n === 38 ? 'fg' : 'bg';
      const kind = parts[i + 1];
      if (kind === 5 && parts[i + 2] != null) {
        next[target] = `256:${parts[i + 2]}`;
        i += 2;
      } else if (kind === 2 && parts[i + 4] != null) {
        next[target] = `rgb:${parts[i + 2]},${parts[i + 3]},${parts[i + 4]}`;
        i += 4;
      }
      continue;
    }
  }
  return next;
}

function wrapSpan(text, attrs) {
  const classes = [];
  const style = [];
  if (attrs.fg != null) pushColor(classes, style, 'fg', attrs.fg);
  if (attrs.bg != null) pushColor(classes, style, 'bg', attrs.bg);
  if (attrs.bold)      classes.push('ansi-bold');
  if (attrs.dim)       classes.push('ansi-dim');
  if (attrs.italic)    classes.push('ansi-italic');
  if (attrs.underline) classes.push('ansi-underline');
  if (attrs.reverse)   classes.push('ansi-reverse');
  if (attrs.strike)    classes.push('ansi-strike');
  if (!classes.length && !style.length) return text;
  const cls = classes.length ? ` class="${classes.join(' ')}"` : '';
  const sty = style.length ? ` style="${style.join(';')}"` : '';
  return `<span${cls}${sty}>${text}</span>`;
}

function pushColor(classes, style, kind, value) {
  if (typeof value === 'number') {
    classes.push(`ansi-${kind}-${value}`);
    return;
  }
  if (typeof value === 'string' && value.startsWith('rgb:')) {
    const [r, g, b] = value.slice(4).split(',').map(Number);
    const prop = kind === 'fg' ? 'color' : 'background-color';
    style.push(`${prop}:rgb(${r},${g},${b})`);
    return;
  }
  if (typeof value === 'string' && value.startsWith('256:')) {
    const n = Number(value.slice(4));
    const rgb = xterm256(n);
    if (!rgb) return;
    const prop = kind === 'fg' ? 'color' : 'background-color';
    style.push(`${prop}:rgb(${rgb[0]},${rgb[1]},${rgb[2]})`);
  }
}

// xterm 256-colour palette — standard 16 + 6x6x6 cube + 24-step
// greyscale. Callers with numbers 0–15 take the 16-colour path
// above; 16–255 resolve here.
function xterm256(n) {
  if (n < 0 || n > 255) return null;
  if (n < 16) {
    // 16 basic colours (approximate xterm values).
    const basic = [
      [0,0,0],[170,0,0],[0,170,0],[170,85,0],
      [0,0,170],[170,0,170],[0,170,170],[170,170,170],
      [85,85,85],[255,85,85],[85,255,85],[255,255,85],
      [85,85,255],[255,85,255],[85,255,255],[255,255,255],
    ];
    return basic[n];
  }
  if (n < 232) {
    const c = n - 16;
    const step = (v) => (v === 0 ? 0 : 55 + 40 * v);
    const r = Math.floor(c / 36);
    const g = Math.floor((c % 36) / 6);
    const b = c % 6;
    return [step(r), step(g), step(b)];
  }
  const v = 8 + (n - 232) * 10;
  return [v, v, v];
}
