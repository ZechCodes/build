// Shared diff-table markup for the review surfaces (task diff tab + the
// read-only external-worktree browse view). Extracted verbatim from task.js's
// renderDiffTab so both consumers render identical rows. Every path and code
// line is escaped.

import { esc } from "./text.js";
import { ICON_CHECK, ICON_EXTERNAL_LINK, ICON_MESSAGE_SQUARE } from "./icons.js";
import { highlightCode, langForPath } from "./highlight.js";
import { isDotenvPath, maskedDiffCellHtml } from "./secrets.js";
import { fileKey, firstLineOf, untouchedFold } from "./diff.js";
import { groupNoiseFiles, noiseGroupLabel } from "./changesModel.js";
import { editedTimeLabel, editedTimestamp } from "./editedTime.js";
import { DIFF_ROW_HEIGHT, fileBodyIsVisible, rowWindowFor, ROW_WINDOW_SIZE } from "./diffWindow.js";
import { sortDiffFiles } from "./diffSort.js";

const sortedWhenRequested = (files, order) => (order ? sortDiffFiles(files, order) : files);

const MAX_HIGHLIGHT_LINE_LENGTH = 20_000;

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
  // A generated/minified line can be megabytes long. Prism's token walk is
  // superlinear for some grammars; plain escaping keeps every byte readable
  // and copyable without letting one row monopolize navigation.
  if (text.length > MAX_HIGHLIGHT_LINE_LENGTH) return esc(text);
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
export function diffRowsHtml(rows, lang = null, { maskDotenv = false, rowOffset = 0 } = {}) {
  return rows
    .map((r, index) => {
      const rowIndex = rowOffset + index + 1;
      if (r.t !== "hunk")
        return `<tr class="${r.t}" aria-rowindex="${rowIndex}" data-ln="${r.n ?? r.o ?? ""}" data-side="${r.t === "del" ? "old" : "new"}" data-old-line="${r.o ?? ""}" data-new-line="${r.n ?? ""}"><td class="ln">${r.o ?? ""}</td><td class="ln">${r.n ?? ""}</td><td class="code">${codeCellHtml(r.text, lang, maskDotenv)}</td></tr>`;
      return `<tr class="hunk" aria-rowindex="${rowIndex}"><td class="ln"></td><td class="ln"></td><td class="code">${codeCellHtml(r.text, lang, false)}</td></tr>`;
    })
    .join("");
}

/** HTML for parsed diff files (core/diff.js parseDiff output). The path is
 *  escaped; code cells are syntax-highlighted by the file's extension. The
 *  table sits inside a .dscroll box so the code scrolls horizontally while the
 *  .fhead header stays fixed.
 *
 *  Re-review options (all opt-in; omitting them keeps the output byte-identical
 *  so the poll-repaint freeze contract holds): `changedSince` is a Set of paths
 *  that moved since the reviewer's last pass (an amber "changed since your
 *  review" chip); `approved` is a Set of paths the reviewer has approved (those
 *  files render `collapsed` instead of `capped` — collapsed wins); and
 *  `approvable` adds the per-file Approve toggle to each header.
 *
 *  `selectable` puts a checkbox ahead of each path and `selected` says which
 *  are ticked: the selection is what the surface's bulk verbs act on.
 *
 *  `fileMenu` puts the file's own destructive verbs behind a ⋯ in the header —
 *  where per-file discard lives now that the stage checkboxes are gone (commit
 *  is commit-all). It is `{ openPath, pendingConfirm }`: only the named file's
 *  menu is open, and a matching `discard:<path>` confirm renders armed, so the
 *  existing two-click confirm idiom is what fires it. */
/** How one file of a stack is rendered: the caller's own entry renderer where
 *  it has one (the fold-aware entry), the whole diff otherwise. */
const fileHtmlFor = (options) => options.renderFile || diffFileHtml;

export function diffFilesHtml(files, options = {}) {
  const renderFile = fileHtmlFor(options);
  return files.map((file) => renderFile(file, options)).join("");
}

const FOLD_CLASS = { open: "", shut: "collapsed", capped: "capped" };

/** Which of the three folds a file is in: the one the caller has already
 *  decided, else what the reader last pressed, else the untouched default
 *  (capped, or shut for a file they have ticked off). One rule, so a file's
 *  class and its body can never disagree about how folded it is. */
export function fileFoldOf(file, { fold = null, folds = null, approved = null } = {}) {
  if (fold) return fold;
  const key = fileKey(file);
  return folds ? folds.foldOf(key, { approved }) : untouchedFold(key, approved);
}

function foldClassOf(file, options) {
  return FOLD_CLASS[fileFoldOf(file, options)];
}

function commentButtonHtml(commentable) {
  return commentable
    ? `<button class="fcmt" title="Comment on this file" aria-label="Comment on this file">${ICON_MESSAGE_SQUARE}</button>`
    : "";
}

function changedChipHtml(file, changedSince) {
  return changedSince && changedSince.has(file.path) ? `<span class="fchanged">changed since your review</span>` : "";
}

/// The reviewer's verdict on one file, as a toggle rather than a checkbox: a
/// checkbox is a setting, and approving is something you DO. Pressed state
/// rides `aria-pressed`, so the control says the same thing to a screen reader
/// as the fill says to everyone else.
///
/// An approved file collapses (core/diff.js `untouchedFold`), which is the
/// whole point of saying so: the stack shortens as the reviewer works down it.
function approveToggleHtml(file, { approvable, approved }) {
  if (!approvable) return "";
  const pressed = Boolean(approved && approved.has(file.path));
  const label = pressed ? "Approved" : "Approve";
  return `<button class="fapprove" data-key="${esc(fileKey(file))}" aria-pressed="${pressed}" aria-label="${label}" title="${label}">${ICON_CHECK}</button>`;
}

function editedTimeHtml(editedAt) {
  const timestamp = editedTimestamp(editedAt);
  if (timestamp === null) return "";
  return `<time class="fedited" data-edited-at="${timestamp}" datetime="${new Date(timestamp).toISOString()}">${editedTimeLabel(timestamp)}</time>`;
}

/// The box that puts a file in the surface's selection — what its bulk verbs
/// act on, and what a commit narrows to when anything is ticked. Ahead of the
/// path, because it names the row rather than acting on it.
function selectBoxHtml(file, { selectable, selected }) {
  if (!selectable) return "";
  const checked = selected && selected.has(file.path) ? " checked" : "";
  return `<input type="checkbox" class="fselect-box" data-key="${esc(fileKey(file))}" aria-label="Select ${esc(file.path)}"${checked}/>`;
}

/** One file's header: its path, its status, its weights, and every affordance
 *  the surface offers on it. */
export function fileHeadHtml(file, options) {
  return `<div class="fhead"><span class="fidentity">${selectBoxHtml(file, options)}<span class="fpath">${esc(file.path)}</span></span><span class="fmeta"><span class="fb ${file.status}">${file.status}</span>
        <span class="pm"><span class="a">+${file.add}</span> <span class="d">−${file.del}</span></span>${editedTimeHtml(file.editedAt)}${changedChipHtml(file, options.changedSince)}</span><span class="factions">${approveToggleHtml(file, options)}${openFileButtonHtml(file, options.openable)}${commentButtonHtml(options.commentable)}${fileMenuHtml(file, options.fileMenu)}</span></div>`;
}

// The two boxes a file's body can sit in: the scrolling one the collapse rule
// hides, and the peek it leaves on screen.
const BODY_BOX = "dscroll";
const PEEK_BOX = "dpeek";

function spacerRowHtml(className, rows) {
  return rows > 0 ? `<tr class="drow-spacer ${className}" aria-hidden="true"><td colspan="3" style="height:${rows * DIFF_ROW_HEIGHT}px"></td></tr>` : "";
}

function diffTableBoxHtml(boxClass, file, window = { start: 0, end: file.rows.length }, preserveExtent = false) {
  const shown = file.rows.slice(window.start, window.end);
  const before = preserveExtent ? spacerRowHtml("before", window.start) : "";
  const after = preserveExtent ? spacerRowHtml("after", file.rows.length - window.end) : "";
  return `<div class="${boxClass}" data-row-count="${file.rows.length}"><table aria-rowcount="${file.rows.length}">${before}${diffRowsHtml(shown, langForPath(file.path), {
          maskDotenv: isDotenvPath(file.path),
          rowOffset: window.start,
        })}${after}</table></div>`;
}

/** One file's diff table, every row of it, in the box the collapse rule hides. */
export function fileBodyHtml(file, options) {
  return diffTableBoxHtml(BODY_BOX, file, rowWindowFor(file, "open", options.viewport));
}

/** The rows a collapsed file keeps on screen: the same table in the box that
 *  survives the collapse — a peek is what a folded file is for. */
export function filePeekHtml(file, options) {
  return diffTableBoxHtml(PEEK_BOX, file);
}

function virtualFileBodyHtml(file, fold) {
  const rows = file.rows.length;
  const previewRows = fold === "open" ? Math.min(rows, ROW_WINDOW_SIZE) : rowWindowFor(file, fold).end;
  const box = fold === "shut" ? PEEK_BOX : BODY_BOX;
  return `<div class="${box} dvirtual" data-row-count="${rows}" style="height:${previewRows * DIFF_ROW_HEIGHT}px"><table aria-rowcount="${rows}">${spacerRowHtml("virtual", previewRows)}</table></div>`;
}

/** Fold-aware contents shared by the direct aggregate renderer and cached-body
 * entries. It deliberately excludes the outer keyed frame. */
export function fileContentHtml(file, fold, options = {}) {
  if (!file.rows) return fileNoticeHtml(fold === "shut" ? "expand to load this file" : "loading…");
  if (!fileBodyIsVisible(file, fold, options.viewport)) return virtualFileBodyHtml(file, fold);
  const window = rowWindowFor(file, fold, options.viewport);
  // A viewport-managed open body is always an inner scroller. That keeps a
  // 100-row body and its offscreen placeholder at the same capped height just
  // as it does a 100,000-row body.
  const windowed = fold === "open" && Boolean(options.viewport);
  const box = `${fold === "shut" ? PEEK_BOX : BODY_BOX}${windowed ? " dwindow" : ""}`;
  return diffTableBoxHtml(box, file, window, windowed);
}

/** What a file shows where its rows are not there to show: one dim line, in the
 *  peek's box, so a collapsed file says how to get them. */
export function fileNoticeHtml(label) {
  return `<div class="${PEEK_BOX}"><div class="dload">${esc(label)}</div></div>`;
}

/** A file's header and the caller's choice of body, in the fold the reader put
 *  it in. The fold-aware entry (core/fileEntries.js) hands a preview or a
 *  loading line where this module hands the whole diff. */
export function fileFrameHtml(file, options, bodyHtml) {
  const classes = ["file", foldClassOf(file, options)].filter(Boolean).join(" ");
  return `
      <div class="${classes}" data-key="${esc(fileKey(file))}">${fileHeadHtml(file, options)}
        ${bodyHtml}
        <div class="diff-expand" aria-hidden="true">Expand full diff ↓</div></div>`;
}

export function diffFileHtml(file, options = {}) {
  const fold = fileFoldOf(file, options);
  return fileFrameHtml(file, options, fileContentHtml(file, fold, options));
}

function openFileButtonHtml(file, openable) {
  if (!openable) return "";
  return `<button class="fopen" data-open-file="${esc(file.path)}" data-new-line="${firstLineOf(file)}" title="Open this file in Files" aria-label="Open this file in Files">${ICON_EXTERNAL_LINK}</button>`;
}

export const FILE_ELEMENT = ".file[data-key]";

export function pressedOpenFile(target, openFile) {
  const control = target.closest("[data-open-file]");
  if (!control || !openFile) return false;
  openFile({ path: control.dataset.openFile, line: Number(control.dataset.newLine) || null });
  return true;
}

/** Whether a press landed on a control rather than on the file around it. A
 *  button, a checkbox or its label does its own thing, and the fold never reads
 *  the same press as "show me the rest of this file". */
const pressedAControl = (target) => Boolean(target.closest("button, input, label"));

export function pressedFold(target, folds, approved = null) {
  const file = folds ? target.closest(FILE_ELEMENT) : null;
  if (!file || pressedAControl(target)) return false;
  const key = file.dataset.key;
  if (target.closest(".fhead")) {
    folds.press(key, { approved });
    return true;
  }
  if (folds.foldOf(key, { approved }) !== "capped") return false;
  folds.openBody(key);
  return true;
}

function fileMenuHtml(file, fileMenu) {
  if (!fileMenu) return "";
  const key = fileKey(file);
  const open = fileMenu.openPath === file.path;
  const armed = fileMenu.pendingConfirm === `discard:${file.path}`;
  const menu = open
    ? `<div class="fmenu-pop"><button class="btn mini danger gitdiscard${armed ? " armed" : ""}" data-key="${esc(key)}">${armed ? "Discard changes?" : "Discard changes"}</button></div>`
    : "";
  return `<span class="fmenu-host"><button class="fmenu" data-key="${esc(key)}" title="File actions" aria-expanded="${open}">⋯</button>${menu}</span>`;
}

export function diffStackHtml(files, options = {}) {
  return diffStackEntries(files, options)
    .map((entry) => entry.html)
    .join("");
}

export function diffStackEntries(
  files,
  { noiseExpanded = false, sortOrder = null, empty = "No file changes.", ...fileOptions } = {},
) {
  const grouped = groupNoiseFiles(files);
  if (!grouped.files.length && !grouped.noise.length)
    return [{ key: "empty", html: `<div class="empty">${esc(empty)}</div>` }];
  const entries = grouped.files.length ? fileStackEntries(grouped.files, fileOptions, sortOrder) : [];
  if (!grouped.noise.length) return entries;
  const noise = sortedWhenRequested(grouped.noise, sortOrder);
  return [...entries, { key: "noise", html: noiseGroupHtml(noise, noiseExpanded, fileOptions) }];
}

function noiseGroupHtml(noise, noiseExpanded, fileOptions) {
  return `<div class="noisegroup${noiseExpanded ? " open" : ""}">
    <button class="noisehead" aria-expanded="${noiseExpanded}"><span class="disclosure-caret" aria-hidden="true">${noiseExpanded ? "▾" : "▸"}</span> ${noiseGroupLabel(noise.length)}</button>
    ${noiseExpanded ? `<div class="noisefiles">${diffFilesHtml(noise, fileOptions)}</div>` : ""}</div>`;
}

function fileStackEntries(files, options, sortOrder) {
  const renderFile = fileHtmlFor(options);
  return sortedWhenRequested(files, sortOrder).map((file) => ({ key: fileKey(file), html: renderFile(file, options) }));
}

export function stackClaims({ comments, openFile, folds, approved = () => null, repaint }) {
  return [
    (event) => {
      const layer = comments();
      return Boolean(layer && layer.handleClick(event));
    },
    (event) => pressedOpenFile(event.target, openFile()),
    (event) => {
      if (!pressedFold(event.target, folds(), approved())) return false;
      repaint();
      return true;
    },
  ];
}
