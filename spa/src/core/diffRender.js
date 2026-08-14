// Shared diff-table markup for the review surfaces (task diff tab + the
// read-only external-worktree browse view). Extracted verbatim from task.js's
// renderDiffTab so both consumers render identical rows. Every path and code
// line is escaped.

import { esc } from "./text.js";
import { highlightCode, langForPath } from "./highlight.js";
import { isDotenvPath, maskedDiffCellHtml } from "./secrets.js";
import { groupNoiseFiles, noiseGroupLabel } from "./changesModel.js";

/** The code-cell HTML for one diff row. On a dotenv file a secret-like line is
 *  masked (a click-to-reveal spoiler span, both old and new values independent);
 *  every other line is syntax-highlighted as before. The masking is pure and
 *  deterministic per line, so the review surfaces' poll-repaint freeze contract
 *  (unchanged patch → identical HTML) is preserved. */
function codeCellHtml(text, lang, maskDotenv) {
  if (maskDotenv) {
    const masked = maskedDiffCellHtml(text);
    if (masked !== null) return masked;
  }
  return highlightCode(text, lang);
}

/** Table rows for one parsed file's diff (core/diff.js row objects). The two
 *  td.ln columns and the data-ln attribute are the review-surface row contract
 *  (comment anchoring, line-highlight) and stay byte-identical; only td.code's
 *  innerHTML is syntax-highlighted. `lang` is a Prism language id (from
 *  langForPath) or null → each code cell is escaped plain text. Highlighting is
 *  per-line (each row tokenized on its own) — an accepted tradeoff for a
 *  multi-line grammar, since diff rows arrive one line at a time. `maskDotenv`
 *  (set by the caller for a dotenv file path) masks secret-like line content. */
export function diffRowsHtml(rows, lang = null, { maskDotenv = false } = {}) {
  return rows
    .map((r) =>
      r.t === "hunk"
        ? `<tr class="hunk"><td class="ln"></td><td class="ln"></td><td class="code">${highlightCode(r.text, lang)}</td></tr>`
        : `<tr class="${r.t}" data-ln="${r.n ?? r.o ?? ""}" data-side="${r.t === "del" ? "old" : "new"}" data-old-line="${r.o ?? ""}" data-new-line="${r.n ?? ""}"><td class="ln">${r.o ?? ""}</td><td class="ln">${r.n ?? ""}</td><td class="code">${codeCellHtml(r.text, lang, maskDotenv)}</td></tr>`,
    )
    .join("");
}

/** HTML for parsed diff files (core/diff.js parseDiff output). The path is
 *  escaped; code cells are syntax-highlighted by the file's extension. The
 *  table sits inside a .dscroll box so the code scrolls horizontally while the
 *  .fhead header stays fixed.
 *
 *  Folding contract (wired by the mounting view): every file starts `capped`
 *  (max-height + fade); a click on the capped body expands it, a click on the
 *  .fhead toggles `collapsed` (header only). `commentable` adds the
 *  whole-file comment control to the header.
 *
 *  Re-review options (all opt-in; omitting them keeps the output byte-identical
 *  so the poll-repaint freeze contract holds): `changedSince` is a Set of paths
 *  that moved since the reviewer's last pass (an amber "changed since your
 *  review" chip); `viewed` is a Set of paths the reviewer ticked off (those
 *  files render `collapsed` instead of `capped` — collapsed wins); and
 *  `withViewedToggle` adds the per-file "Viewed" checkbox to each header.
 *
 *  `fileMenu` puts the file's own destructive verbs behind a ⋯ in the header —
 *  where per-file discard lives now that the stage checkboxes are gone (commit
 *  is commit-all). It is `{ openPath, pendingConfirm }`: only the named file's
 *  menu is open, and a matching `discard:<path>` confirm renders armed, so the
 *  existing two-click confirm idiom is what fires it. */
export function diffFilesHtml(
  files,
  { commentable = false, changedSince = null, viewed = null, withViewedToggle = false, fileMenu = null } = {},
) {
  const commentButton = commentable ? `<button class="fcmt" title="Comment on this file">✎</button>` : "";
  return files
    .map((f) => {
      const lang = langForPath(f.path);
      const isViewed = viewed ? viewed.has(f.path) : false;
      const foldClass = isViewed ? "collapsed" : "capped";
      const changedChip = changedSince && changedSince.has(f.path) ? `<span class="fchanged">changed since your review</span>` : "";
      const viewedToggle = withViewedToggle
        ? `<label class="fviewed"><input type="checkbox" class="fviewed-box" data-file="${esc(f.path)}"${isViewed ? " checked" : ""}/> Viewed</label>`
        : "";
      return `
      <div class="file ${foldClass}" data-file="${esc(f.path)}"><div class="fhead"><span class="fpath">${esc(f.path)}</span><span class="fb ${f.status}">${f.status}</span>
        <span class="pm"><span class="a">+${f.add}</span> <span class="d">−${f.del}</span></span>${changedChip}${viewedToggle}${commentButton}${fileMenuHtml(f.path, fileMenu)}</div>
        <div class="dscroll"><table>${diffRowsHtml(f.rows, lang, { maskDotenv: isDotenvPath(f.path) })}</table></div>
        <div class="diff-expand" aria-hidden="true">Expand full diff ↓</div></div>`;
    })
    .join("");
}

/** The file header's ⋯ and, when this file's menu is the open one, its verbs.
 *  Today that is one verb — discard — carrying the shared inline confirm. */
function fileMenuHtml(path, fileMenu) {
  if (!fileMenu) return "";
  const open = fileMenu.openPath === path;
  const armed = fileMenu.pendingConfirm === `discard:${path}`;
  const menu = open
    ? `<div class="fmenu-pop"><button class="btn mini danger gitdiscard${armed ? " armed" : ""}" data-path="${esc(path)}">${armed ? "Discard changes?" : "Discard changes"}</button></div>`
    : "";
  return `<span class="fmenu-host"><button class="fmenu" data-path="${esc(path)}" title="File actions" aria-expanded="${open}">⋯</button>${menu}</span>`;
}

/** One changeset, stacked: every readable file as a full diff, then whatever is
 *  machine noise (lockfiles, caches, Build metadata) as ONE collapsed group at
 *  the bottom with a count line. Noise is never filtered away — the doc's rule
 *  is collapse, never hide — so a reviewer can always open it.
 *  `noiseExpanded` is the caller's persisted disclosure state; every other
 *  option passes straight through to diffFilesHtml. */
export function diffStackHtml(files, { noiseExpanded = false, ...fileOptions } = {}) {
  const grouped = groupNoiseFiles(files);
  if (!grouped.files.length && !grouped.noise.length) return '<div class="empty">No file changes.</div>';
  const primary = grouped.files.length ? diffFilesHtml(grouped.files, fileOptions) : "";
  if (!grouped.noise.length) return primary;
  return `${primary}<div class="noisegroup${noiseExpanded ? " open" : ""}">
    <button class="noisehead" aria-expanded="${noiseExpanded}">${noiseExpanded ? "▾" : "▸"} ${noiseGroupLabel(grouped.noise.length)}</button>
    ${noiseExpanded ? `<div class="noisefiles">${diffFilesHtml(grouped.noise, fileOptions)}</div>` : ""}</div>`;
}
