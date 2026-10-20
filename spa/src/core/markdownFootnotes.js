// Footnote definitions are collected before rendering for forward references.
// Lines are blanked rather than deleted, preserving task-edit source offsets.
// IDs never contain author text: scopes are code-point encoded and note/ref
// numbers are generated locally. Only these fixed fragment URLs are emitted.
import { fenceOpening, fenceCloses } from "./markdownFences.js";

const DEFINITION = /^ {0,3}\[\^([A-Za-z0-9_-]+)\]:[ \t]*(.*)$/;
const CONTINUATION = /^(?: {4}|\t)/;
const encode = (text) => Array.from(String(text), (character) => character.codePointAt(0).toString(16)).join("-");

// The unscoped API has no surface identity; a deterministic pair of hashes
// keeps IDs compact. Surfaces with multiple documents pass taskLabelScope.
function sourceScope(source) {
  let first = 2166136261;
  let second = 5381;
  for (let index = 0; index < source.length; index += 1) {
    first = Math.imul(first ^ source.charCodeAt(index), 16777619);
    second = Math.imul(second, 33) ^ source.charCodeAt(index);
  }
  return `${(first >>> 0).toString(16)}-${(second >>> 0).toString(16)}`;
}

function definitionEnd(lines, at) {
  let end = at + 1;
  let next = end;
  while (next < lines.length) {
    if (!lines[next].trim()) { next += 1; continue; }
    if (!CONTINUATION.test(lines[next])) break;
    next += 1;
    end = next;
  }
  return end;
}

function definitionAt(lines, offsets, at, match) {
  const end = definitionEnd(lines, at);
  const margin = lines[at].length - match[2].length;
  const content = [match[2]];
  const starts = [offsets[at] + margin];
  for (let next = at + 1; next < end; next += 1) {
    const removed = CONTINUATION.exec(lines[next])?.[0].length || 0;
    content.push(lines[next].slice(removed));
    starts.push(offsets[next] + removed);
  }
  return { lines: content, offsets: starts, end };
}

// Definitions belong to the document margin. An indented list body is read
// in its own bounded block context, so its unclosed fences must not consume
// definitions that follow the list, nor turn fenced examples into notes.
function listBodyEnd(lines, at) {
  const item = /^( *)(?:[-*+]|\d{1,9}[.)]) +/.exec(lines[at]);
  if (!item) return at;
  let next = at + 1;
  while (next < lines.length) {
    if (lines[next].trim() && lines[next].length - lines[next].trimStart().length <= item[1].length) break;
    next += 1;
  }
  return next - 1;
}

export function collectFootnotes(lines, offsets, source, scope) {
  const definitions = new Map();
  let fence = null;
  for (let at = 0; at < lines.length; at += 1) {
    if (fence) {
      if (fenceCloses(lines[at], fence)) fence = null;
      continue;
    }
    fence = fenceOpening(lines[at]);
    if (fence) continue;
    const listEnd = listBodyEnd(lines, at);
    if (listEnd > at) { at = listEnd; continue; }
    const match = DEFINITION.exec(lines[at]);
    if (!match) continue;
    const definition = definitionAt(lines, offsets, at, match);
    if (!definitions.has(match[1])) definitions.set(match[1], definition);
    for (; at < definition.end; at += 1) lines[at] = "";
    at -= 1;
  }
  return footnoteRenderer(definitions, scope == null ? sourceScope(source) : encode(scope));
}

function footnoteRenderer(definitions, scope) {
  const used = new Map();
  const noteId = (number) => `md-fn:${scope}:${number}`;
  const refId = (number, count) => `md-fnref:${scope}:${number}:${count}`;
  const reference = (id) => {
    if (!definitions.has(id)) return null;
    if (!used.has(id)) used.set(id, { number: used.size + 1, count: 0 });
    const note = used.get(id);
    note.count += 1;
    return `<sup class="md-footnote-ref"><a id="${refId(note.number, note.count)}" href="#${noteId(note.number)}" aria-label="Footnote ${note.number}">${note.number}</a></sup>`;
  };
  const html = (render) => {
    if (!used.size) return "";
    const notes = Array.from(used, ([id, note]) => {
      const definition = definitions.get(id);
      const body = render(definition.lines, definition.offsets);
      const backlinks = Array.from({ length: note.count }, (_, index) => `<a class="md-footnote-backref" href="#${refId(note.number, index + 1)}" aria-label="Back to reference ${note.number}${note.count > 1 ? ` (${index + 1})` : ""}">↩</a>`).join(" ");
      return `<li id="${noteId(note.number)}">${body} ${backlinks}</li>`;
    }).join("");
    return `<section class="md-footnotes" aria-label="Footnotes"><ol>${notes}</ol></section>`;
  };
  return { reference, html };
}
