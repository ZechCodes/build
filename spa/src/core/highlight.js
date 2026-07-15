// Syntax highlighting for the diff and file-source surfaces. We import Prism's
// CORE only (prismjs/components/prism-core — NOT the default "prismjs" entry, so
// Prism never auto-highlights the live DOM) and register a fixed grammar set,
// then call Prism.highlight explicitly per line.
//
// SECURITY: the output is XSS-safe. Prism.highlight escapes `<` and `&` in token
// text (enough that no untrusted source can inject a live tag), and the
// unknown-language fallback runs the source through esc() — which additionally
// escapes `>`, `"`, and `'`. The token markup is foreground-only (styles.css),
// so the add/del diff-row background tints are left untouched.

import Prism from "prismjs/components/prism-core";
import "prismjs/components/prism-markup";
import "prismjs/components/prism-clike";
import "prismjs/components/prism-css";
import "prismjs/components/prism-javascript";
import "prismjs/components/prism-jsx";
import "prismjs/components/prism-typescript";
import "prismjs/components/prism-tsx";
import "prismjs/components/prism-python";
import "prismjs/components/prism-rust";
import "prismjs/components/prism-json";
import "prismjs/components/prism-bash";
import "prismjs/components/prism-yaml";
import "prismjs/components/prism-toml";
import "prismjs/components/prism-markdown";
import { esc } from "./text.js";

// File extension (lower-cased) → Prism language id. Only the grammars registered
// above appear here; anything else falls through to the escaped plain-text path.
const EXTENSION_LANGUAGE = {
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  jsx: "jsx",
  ts: "typescript",
  tsx: "tsx",
  py: "python",
  rs: "rust",
  css: "css",
  html: "markup",
  htm: "markup",
  xml: "markup",
  svg: "markup",
  json: "json",
  sh: "bash",
  bash: "bash",
  yaml: "yaml",
  yml: "yaml",
  toml: "toml",
  md: "markdown",
  markdown: "markdown",
};

/** Pure: the Prism language id for a path's extension, or null when unknown or
 *  extension-less. The extension is the run of non-dot, non-separator chars
 *  after the final dot, so "a.dir/file" (dot in a directory, none in the name)
 *  correctly yields null. */
export function langForPath(path) {
  const match = /\.([^./\\]+)$/.exec((path ?? "").toString());
  if (!match) return null;
  return EXTENSION_LANGUAGE[match[1].toLowerCase()] ?? null;
}

/** Pure: highlight one source string as HTML. A known, registered `lang` yields
 *  Prism token markup (which escapes `<`/`&`); an unknown lang (or null) falls
 *  back to a fully-escaped plain-text render. Never returns unescaped source. */
export function highlightCode(text, lang) {
  const source = (text ?? "").toString();
  return lang && Prism.languages[lang] ? Prism.highlight(source, Prism.languages[lang], lang) : esc(source);
}
