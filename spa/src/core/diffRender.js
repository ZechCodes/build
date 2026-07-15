// Shared diff-table markup for the review surfaces (task diff tab + the
// read-only external-worktree browse view). Extracted verbatim from task.js's
// renderDiffTab so both consumers render identical rows. Every path and code
// line is escaped.

import { esc } from "./text.js";
import { highlightCode, langForPath } from "./highlight.js";

/** Table rows for one parsed file's diff (core/diff.js row objects). The two
 *  td.ln columns and the data-ln attribute are the review-surface row contract
 *  (comment anchoring, line-highlight) and stay byte-identical; only td.code's
 *  innerHTML is syntax-highlighted. `lang` is a Prism language id (from
 *  langForPath) or null → each code cell is escaped plain text. Highlighting is
 *  per-line (each row tokenized on its own) — an accepted tradeoff for a
 *  multi-line grammar, since diff rows arrive one line at a time. */
export function diffRowsHtml(rows, lang = null) {
  return rows
    .map((r) =>
      r.t === "hunk"
        ? `<tr class="hunk"><td class="ln"></td><td class="ln"></td><td class="code">${highlightCode(r.text, lang)}</td></tr>`
        : `<tr class="${r.t}" data-ln="${r.n ?? r.o ?? ""}"><td class="ln">${r.o ?? ""}</td><td class="ln">${r.n ?? ""}</td><td class="code">${highlightCode(r.text, lang)}</td></tr>`,
    )
    .join("");
}

/** HTML for parsed diff files (core/diff.js parseDiff output). The path is
 *  escaped; code cells are syntax-highlighted by the file's extension. The
 *  table sits inside a .dscroll box so the code scrolls horizontally while the
 *  .fhead header stays fixed. */
export function diffFilesHtml(files) {
  return files
    .map((f) => {
      const lang = langForPath(f.path);
      return `
      <div class="file" data-file="${esc(f.path)}"><div class="fhead"><span>${esc(f.path)}</span><span class="fb ${f.status}">${f.status}</span>
        <span class="pm"><span class="a">+${f.add}</span> <span class="d">−${f.del}</span></span></div>
        <div class="dscroll"><table>${diffRowsHtml(f.rows, lang)}</table></div></div>`;
    })
    .join("");
}
