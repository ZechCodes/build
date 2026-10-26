// Fences share the file/diff highlighter's fixed Prism grammar set. The info
// string is displayed as escaped text; only known language names enter classes.
import { esc } from "./text.js";
import { highlightCode, langForPath } from "./highlight.js";

const FENCE = /^ {0,3}(`{3,}|~{3,})([^\n]*)$/;
const LANGUAGES = new Set(["markup", "clike", "css", "javascript", "jsx", "typescript", "tsx", "python", "rust", "json", "bash", "yaml", "toml", "markdown"]);
const ALIASES = new Map([["shell", "bash"], ["shellscript", "bash"], ["console", "bash"]]);

export function fenceOpening(line) {
  const match = FENCE.exec(line ?? "");
  if (!match || (match[1][0] === "`" && match[2].includes("`"))) return null;
  return { mark: match[1][0], length: match[1].length, indent: line.length - line.trimStart().length, label: match[2].trim().split(/\s+/)[0] };
}

export function fenceCloses(line, opening) {
  const match = /^ {0,3}(`+|~+)[ \t]*$/.exec(line);
  return Boolean(match && match[1][0] === opening.mark && match[1].length >= opening.length);
}

function languageOf(label) {
  if (LANGUAGES.has(label)) return label;
  const alias = ALIASES.get(label) || langForPath(`code.${label}`);
  return LANGUAGES.has(alias) ? alias : null;
}

export function readFence(lines, at) {
  const opening = fenceOpening(lines[at]);
  let next = at + 1;
  const code = [];
  for (; next < lines.length && !fenceCloses(lines[next], opening); next += 1) {
    const indent = Math.min(opening.indent, lines[next].length - lines[next].trimStart().length);
    code.push(`${lines[next].slice(indent)}\n`);
  }
  const language = languageOf(opening.label.toLowerCase());
  const attribute = language ? ` class="language-${language}"` : "";
  const source = code.join("");
  // Prism grammars use backtracking regular expressions. Large source stays
  // readable through the linear escaped path rather than blocking a repaint.
  const highlighted = source.length <= 4096 ? highlightCode(source, language) : esc(source);
  const pre = `<pre class="md-code"><code${attribute}>${highlighted}</code></pre>`;
  const blockClass = opening.label.toLowerCase() === "mermaid" && next < lines.length ? "md-code-block md-mermaid" : "md-code-block";
  const html = opening.label ? `<div class="${blockClass}"><div class="md-code-label">${esc(opening.label)}</div>${pre}</div>` : pre;
  return { html, next: Math.min(next + 1, lines.length) };
}
