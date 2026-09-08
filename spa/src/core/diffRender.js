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
import { planChangesetTriage, triageSummaryLine, overrideDirectionFor } from "./triageModel.js";

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
export function diffRowsHtml(rows, lang = null, { maskDotenv = false, hunkMarks = null, overridable = false } = {}) {
  let hunkIndex = 0;
  return rows
    // eslint-disable-next-line complexity -- ratchet: this callback is at 12, cap 10 — reduce it, then drop this line
    .map((r) => {
      if (r.t !== "hunk")
        return `<tr class="${r.t}" data-ln="${r.n ?? r.o ?? ""}" data-side="${r.t === "del" ? "old" : "new"}" data-old-line="${r.o ?? ""}" data-new-line="${r.n ?? ""}"><td class="ln">${r.o ?? ""}</td><td class="ln">${r.n ?? ""}</td><td class="code">${codeCellHtml(r.text, lang, maskDotenv)}</td></tr>`;
      const mark = hunkMarks ? hunkMarks[hunkIndex] : null;
      hunkIndex++;
      const attributes = mark ? ` data-hunk="${esc(mark.hunk_id || "")}" data-level="${esc(mark.level)}"` : "";
      return `<tr class="hunk"${attributes}><td class="ln"></td><td class="ln"></td><td class="code">${highlightCode(r.text, lang)}${hunkChipHtml(mark, overridable)}</td></tr>`;
    })
    .join("");
}

/** What a hunk row carries besides its code: the pass's claim about it, the
 *  reviewer's answer to that claim, and — where the surface can post one — the
 *  offer to disagree.
 *
 *  The level chip goes on a surfaced critical (carrying the pass's one-line
 *  rationale, on hover as a title and on tap as a revealed line) and on a hunk
 *  the pass never named, so an ordering with holes in it says so rather than
 *  implying every unchipped hunk was read. Nothing else is chipped: a collapsed
 *  group's header already says what its hunks are, and chipping every normal
 *  hunk would be noise over the whole stack. */
function hunkChipHtml(mark, overridable = false) {
  if (!mark) return "";
  if (mark.untriaged)
    return `<span class="hchip untriaged" title="the triage pass did not classify this hunk">untriaged</span>`;
  const rationale = mark.rationale || "";
  const level =
    mark.level === "critical"
      ? `<span class="hchip critical" tabindex="0" title="${esc(rationale)}">critical${
          rationale ? `<span class="hrationale">${esc(rationale)}</span>` : ""
        }</span>`
      : "";
  return level + overrideChipHtml(mark) + (overridable ? overrideButtonHtml(mark) : "");
}

/** The chip that says this hunk sits where it sits because the reviewer said
 *  so, not because the pass did — and, when they left one, why. Without it a
 *  reviewer coming back to a stack could not tell their own corrections from
 *  the pass's reading, and a repaint would look like the pass had changed its
 *  mind. */
function overrideChipHtml(mark) {
  if (!mark.overridden) return "";
  const surfaced = mark.overrideDirection === "surface";
  const note = mark.note || "";
  const title = note || (surfaced ? "you kept this hunk in the stack" : "you collapsed this hunk");
  return `<span class="hchip overridden" tabindex="0" title="${esc(title)}">your call: ${
    surfaced ? "surfaced" : "collapsed"
  }${note ? `<span class="hchip-note">${esc(note)}</span>` : ""}</span>`;
}

/** The offer to disagree, on the hunk the decision was about: a surfaced
 *  critical offers to be collapsed, a collapsed hunk offers to be kept
 *  surfaced, and a hunk the reviewer already moved offers the way back. */
function overrideButtonHtml(mark) {
  const direction = overrideDirectionFor(mark);
  if (!direction) return "";
  const surfacing = direction === "surface";
  return `<button class="toverride" data-hunk="${esc(mark.hunk_id)}" data-direction="${direction}" title="${
    surfacing ? "disagree: this needs reading" : "disagree: this does not need reading"
  }">${surfacing ? "Keep surfaced" : "Collapse"}</button>`;
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
  return `<button class="fapprove" data-key="${esc(fileKey(file))}" aria-pressed="${pressed}">${ICON_CHECK}<span>${
    pressed ? "Approved" : "Approve"
  }</span></button>`;
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
  return `<div class="fhead">${selectBoxHtml(file, options)}<span class="fpath">${esc(file.path)}</span><span class="fb ${file.status}">${file.status}</span>
        <span class="pm"><span class="a">+${file.add}</span> <span class="d">−${file.del}</span></span>${changedChipHtml(file, options.changedSince)}${approveToggleHtml(file, options)}${openFileButtonHtml(file, options.openable)}${commentButtonHtml(options.commentable)}${fileMenuHtml(file, options.fileMenu)}</div>`;
}

// The two boxes a file's body can sit in: the scrolling one the collapse rule
// hides, and the peek it leaves on screen.
const BODY_BOX = "dscroll";
const PEEK_BOX = "dpeek";

function diffTableBoxHtml(boxClass, file, options) {
  return `<div class="${boxClass}"><table>${diffRowsHtml(file.rows, langForPath(file.path), {
          maskDotenv: isDotenvPath(file.path),
          hunkMarks: file.triageHunks || null,
          overridable: options.overridable,
        })}</table></div>`;
}

/** One file's diff table, every row of it, in the box the collapse rule hides. */
export function fileBodyHtml(file, options) {
  return diffTableBoxHtml(BODY_BOX, file, options);
}

/** The rows a collapsed file keeps on screen: the same table in the box that
 *  survives the collapse — a peek is what a folded file is for. */
export function filePeekHtml(file, options) {
  return diffTableBoxHtml(PEEK_BOX, file, options);
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
  const classes = ["file", foldClassOf(file, options), options.sectionClass].filter(Boolean).join(" ");
  return `
      <div class="${classes}" data-key="${esc(fileKey(file))}">${fileHeadHtml(file, options)}
        ${bodyHtml}
        <div class="diff-expand" aria-hidden="true">Expand full diff ↓</div></div>`;
}

export function diffFileHtml(file, options = {}) {
  return fileFrameHtml(file, options, fileBodyHtml(file, options));
}

function openFileButtonHtml(file, openable) {
  if (!openable) return "";
  return `<button class="fopen" data-open-file="${esc(file.path)}" data-new-line="${firstLineOf(file)}" title="Open this file in Files">${ICON_EXTERNAL_LINK}<span>Open File</span></button>`;
}

export const FILE_ELEMENT = ".file[data-key]";

export function pressedOpenFile(target, openFile) {
  const control = target.closest("[data-open-file]");
  if (!control || !openFile) return false;
  openFile({ path: control.dataset.openFile, line: Number(control.dataset.newLine) || null });
  return true;
}

export function pressedFold(target, folds, approved = null) {
  const file = folds ? target.closest(FILE_ELEMENT) : null;
  if (!file) return false;
  const key = file.dataset.key;
  if (target.closest(".fhead") && !target.closest("button, input, label")) {
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

export function diffStackEntries(files, { noiseExpanded = false, review = null, empty = "No file changes.", ...fileOptions } = {}) {
  const grouped = groupNoiseFiles(files);
  if (!grouped.files.length && !grouped.noise.length)
    return [{ key: "empty", html: `<div class="empty">${esc(empty)}</div>` }];
  const entries = grouped.files.length ? reviewStackEntries(grouped.files, review, fileOptions) : [];
  if (!grouped.noise.length) return entries;
  return [...entries, { key: "noise", html: noiseGroupHtml(grouped.noise, noiseExpanded, fileOptions) }];
}

function noiseGroupHtml(noise, noiseExpanded, fileOptions) {
  return `<div class="noisegroup${noiseExpanded ? " open" : ""}">
    <button class="noisehead" aria-expanded="${noiseExpanded}">${noiseExpanded ? "▾" : "▸"} ${noiseGroupLabel(noise.length)}</button>
    ${noiseExpanded ? `<div class="noisefiles">${diffFilesHtml(noise, fileOptions)}</div>` : ""}</div>`;
}

// ---- the triage overlay ----------------------------------------------------
//
// Review prioritization (UX Redesign Decisions, "Review prioritization"): the
// stacked full-file diffs stay exactly what they are, and the overlay only
// decides what order they come in and what starts collapsed. The diff is ground
// truth; this is a reading of it, and the dial turns the reading off.

/** The banner over an overlaid stack: what the pass did (or why there is no
 *  ordering), and the reviewer's dial. */
function triageBarHtml(plan, { dial, offerDial }) {
  const claim =
    plan.status === "none"
      ? `<span class="tuntriaged">untriaged — the full diff, in file order</span>`
      : plan.status === "stale"
        ? `<span class="tstale">triage from an earlier revision — re-triaging</span><span class="tsummary">${esc(triageSummaryLine(plan))}</span>`
        : `<span class="tsummary">${esc(triageSummaryLine(plan))}</span>`;
  const dialButton = offerDial
    ? `<button class="tdial" aria-pressed="${dial}" title="${
        dial ? "order the diff by the triage pass again" : "show the full diff in file order, untriaged"
      }">${dial ? "Show ordered diff" : "Show full diff"}</button>`
    : "";
  return `<div class="triagebar">${claim}${dialButton}</div>`;
}

function triageGroupHeadHtml(section, expanded) {
  const counts = `${section.fileCount} file${section.fileCount === 1 ? "" : "s"} · ${section.hunkCount} hunk${
    section.hunkCount === 1 ? "" : "s"
  }`;
  return `<button class="tgrouphead${expanded ? " open" : ""}" aria-expanded="${expanded}" data-group="${esc(section.name)}">${expanded ? "▾" : "▸"} <span class="tgname">${esc(section.name)}</span> <span class="tgcount">${counts}</span>${
    section.rationale ? `<span class="tgrationale">${esc(section.rationale)}</span>` : ""
  }</button>`;
}

function triageGroupHtml(section, fileOptions) {
  return `<div class="tgroup" data-group="${esc(section.name)}">${triageGroupHeadHtml(section, false)}
    <div class="tgfiles">${diffFilesHtml(section.files, fileOptions)}</div></div>`;
}

function reviewStackEntries(files, review, options) {
  const fileEntries = (list, fileOptions) =>
    list.map((file) => ({ key: fileKey(file), html: fileHtmlFor(fileOptions)(file, fileOptions) }));
  if (!review) return fileEntries(files, options);
  const { triage = null, patch = "", dial = false, expandedGroups = null, overridable = false } = review;
  const fileOptions = { ...options, overridable };
  // The dial renders the untriaged stack, and says so — the pass is still
  // there, and one click puts it back.
  if (dial)
    return [
      { key: "triagebar", html: triageBarHtml({ status: "none", counts: {} }, { dial: true, offerDial: Boolean(triage) }) },
      ...fileEntries(files, fileOptions),
    ];
  const plan = planChangesetTriage({ files, patch, triage });
  const bar = triageBarHtml(plan, { dial: false, offerDial: Boolean(triage) && plan.status !== "none" });
  const opened = (name) => Boolean(expandedGroups && expandedGroups.has(name));
  return [
    { key: "triagebar", html: bar },
    ...plan.sections.flatMap((section) => sectionEntries(section, { opened, fileEntries, fileOptions })),
  ];
}

function sectionEntries(section, { opened, fileEntries, fileOptions }) {
  if (section.kind === "group" && !opened(section.name))
    return [{ key: `group:${section.name}`, html: triageGroupHtml(section, fileOptions) }];
  const sectionClass = section.kind === "group" ? "tgrouped" : `t${section.kind}`;
  const files = fileEntries(section.files, { ...fileOptions, sectionClass });
  const head = sectionHeadHtml(section);
  if (!head) return files;
  const key = section.kind === "group" ? `grouphead:${section.name}` : `sectionhead:${section.kind}`;
  return [{ key, html: head }, ...files];
}

function sectionHeadHtml(section) {
  if (section.kind === "group") return triageGroupHeadHtml(section, true);
  return section.kind === "critical" ? `<div class="tsectionhead">Needs review first</div>` : "";
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
