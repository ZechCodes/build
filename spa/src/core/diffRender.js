// Shared diff-table markup for the review surfaces (task diff tab + the
// read-only external-worktree browse view). Extracted verbatim from task.js's
// renderDiffTab so both consumers render identical rows. Every path and code
// line is escaped.

import { esc } from "./text.js";

/** Table rows for one parsed file's diff (core/diff.js row objects). Escaped. */
export function diffRowsHtml(rows) {
  return rows
    .map((r) =>
      r.t === "hunk"
        ? `<tr class="hunk"><td class="ln"></td><td class="ln"></td><td class="code">${esc(r.text)}</td></tr>`
        : `<tr class="${r.t}" data-ln="${r.n ?? r.o ?? ""}"><td class="ln">${r.o ?? ""}</td><td class="ln">${r.n ?? ""}</td><td class="code">${esc(r.text)}</td></tr>`,
    )
    .join("");
}

/** HTML for parsed diff files (core/diff.js parseDiff output). Escaped. */
export function diffFilesHtml(files) {
  return files
    .map(
      (f) => `
      <div class="file" data-file="${esc(f.path)}"><div class="fhead"><span>${esc(f.path)}</span><span class="fb ${f.status}">${f.status}</span>
        <span class="pm"><span class="a">+${f.add}</span> <span class="d">−${f.del}</span></span></div>
        <table>${diffRowsHtml(f.rows)}</table></div>`,
    )
    .join("");
}
