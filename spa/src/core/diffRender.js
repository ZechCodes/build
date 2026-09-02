// Shared diff-table markup for the review surfaces (task diff tab + the
// read-only external-worktree browse view). Extracted verbatim from task.js's
// renderDiffTab so both consumers render identical rows. Every path and code
// line is escaped.

import { esc } from "./text.js";
import { highlightCode, langForPath } from "./highlight.js";
import { isDotenvPath, maskedDiffCellHtml } from "./secrets.js";
import { fileKey, firstLineOf } from "./diff.js";
import { anchorTop } from "./paintKeepingPlace.js";
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
 *  Folding contract: every file starts `capped` (max-height + fade); the
 *  mounting view's handler answers a press on the capped body by opening the
 *  file and a press on the .fhead by shutting it, both by moving its key in
 *  the sets below. `commentable` adds the whole-file comment control to the
 *  header.
 *
 *  Re-review options (all opt-in; omitting them keeps the output byte-identical
 *  so the poll-repaint freeze contract holds): `changedSince` is a Set of paths
 *  that moved since the reviewer's last pass (an amber "changed since your
 *  review" chip); `viewed` is a Set of paths the reviewer ticked off (those
 *  files render `collapsed` instead of `capped` — collapsed wins); and
 *  `withViewedToggle` adds the per-file "Viewed" checkbox to each header.
 *
 *  Folds are the reader's, and they arrive as state: `expanded` and
 *  `collapsed` are sets of file keys (core/diff.js fileKey) the reader opened
 *  and shut, and `viewed` shuts a file besides ticking its box. Nothing here
 *  reads a class list back — the render is a function of those sets.
 *
 *  `fileMenu` puts the file's own destructive verbs behind a ⋯ in the header —
 *  where per-file discard lives now that the stage checkboxes are gone (commit
 *  is commit-all). It is `{ openPath, pendingConfirm }`: only the named file's
 *  menu is open, and a matching `discard:<path>` confirm renders armed, so the
 *  existing two-click confirm idiom is what fires it. */
export function diffFilesHtml(files, options = {}) {
  return files.map((file) => diffFileHtml(file, options)).join("");
}

/** The fold one file wears, from the reader's state alone: what they shut (or
 *  ticked off as read) is collapsed, what they opened is whole, and everything
 *  else is the capped peek a file starts at. */
function foldClassOf(file, { expanded, collapsed, viewed }) {
  const key = fileKey(file);
  if ((collapsed && collapsed.has(key)) || (viewed && viewed.has(file.path))) return "collapsed";
  return expanded && expanded.has(key) ? "" : "capped";
}

/** One file of a changeset: its header, its rows, and the fold the reader left
 *  it in. `data-key` names the element and the state entry alike; while the
 *  file is open it also carries `data-expanded`, which is what the patch in
 *  core/domPatch.js reads to leave an expansion alone. */
export function diffFileHtml(file, options = {}) {
  const { commentable = false, changedSince = null, viewed = null, withViewedToggle = false, fileMenu = null, overridable = false, openable = false } = options;
  const lang = langForPath(file.path);
  const key = fileKey(file);
  const isViewed = viewed ? viewed.has(file.path) : false;
  const foldClass = foldClassOf(file, options);
  const classes = ["file", foldClass].filter(Boolean).join(" ");
  const openMark = foldClass === "" ? " data-expanded" : "";
  const commentButton = commentable ? `<button class="fcmt" title="Comment on this file">✎</button>` : "";
  const changedChip = changedSince && changedSince.has(file.path) ? `<span class="fchanged">changed since your review</span>` : "";
  const viewedToggle = withViewedToggle
    ? `<label class="fviewed"><input type="checkbox" class="fviewed-box" data-file="${esc(file.path)}"${isViewed ? " checked" : ""}/> Viewed</label>`
    : "";
  return `
      <div class="${classes}" data-file="${esc(file.path)}" data-key="${esc(key)}"${openMark}><div class="fhead"><span class="fpath">${esc(file.path)}</span><span class="fb ${file.status}">${file.status}</span>
        <span class="pm"><span class="a">+${file.add}</span> <span class="d">−${file.del}</span></span>${changedChip}${viewedToggle}${openFileButtonHtml(file, openable)}${commentButton}${fileMenuHtml(file.path, fileMenu)}</div>
        <div class="dscroll"><table>${diffRowsHtml(file.rows, lang, {
          maskDotenv: isDotenvPath(file.path),
          hunkMarks: file.triageHunks || null,
          overridable,
        })}</table></div>
        <div class="diff-expand" aria-hidden="true">Expand full diff ↓</div></div>`;
}

/** The way out of the diff and into the file itself, at the line the diff is
 *  about. Drawn only where the surface has somewhere to send the reader. */
function openFileButtonHtml(file, openable) {
  if (!openable) return "";
  return `<button class="fopen" data-open-file="${esc(file.path)}" data-line="${firstLineOf(file)}" title="Open this file in Files">↗</button>`;
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
 *  option passes straight through to diffFilesHtml.
 *
 *  `review` plugs the triage overlay in (see reviewStackEntries). Omitting it
 *  leaves the output byte-identical to what it always was, which is what a
 *  surface with no triage to render — and the poll-repaint freeze contract —
 *  depends on. */
export function diffStackHtml(files, options = {}) {
  return diffStackEntries(files, options)
    .map((entry) => entry.html)
    .join("");
}

/** Where a repaint of a stack leaves the reader: on the file they were reading.
 *  The markup that names each file is here, so the way to find it again is
 *  here too — both controllers hand this to core/paintKeepingPlace.js. */
export const DIFF_PLACE_KEEPING = {
  opening: (scroller) => !scroller.querySelector(".file[data-key]"),
  policy: anchorTop(".file[data-key]"),
};

/** The same stack as a keyed list: `[{ key, html }]`, one entry per block a
 *  repaint can move — a file (named by its file key), the triage bar, a triage
 *  section or group, the noise group. A controller patches those into a
 *  container with core/patchList.js, so a tick that changed one file leaves
 *  every other block — and the reader's place in it — standing. */
export function diffStackEntries(files, { noiseExpanded = false, review = null, ...fileOptions } = {}) {
  const grouped = groupNoiseFiles(files);
  if (!grouped.files.length && !grouped.noise.length)
    return [{ key: "empty", html: '<div class="empty">No file changes.</div>' }];
  const entries = grouped.files.length ? reviewStackEntries(grouped.files, review, fileOptions) : [];
  if (!grouped.noise.length) return entries;
  return [...entries, { key: "noise", html: noiseGroupHtml(grouped.noise, noiseExpanded, fileOptions) }];
}

/** The machine's own files, under one count line at the bottom of the stack. */
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

/** One collapsed group: its name, the one line that says why its hunks are not
 *  worth the reviewer's attention, and its counts. The diffs inside are ALWAYS
 *  rendered — collapsed, never dropped — so a group is one click from being
 *  read and nothing is missing from the page a reviewer searches. */
function triageGroupHtml(section, { expanded, fileOptions }) {
  const counts = `${section.fileCount} file${section.fileCount === 1 ? "" : "s"} · ${section.hunkCount} hunk${
    section.hunkCount === 1 ? "" : "s"
  }`;
  return `<div class="tgroup${expanded ? " open" : ""}" data-group="${esc(section.name)}">
    <button class="tgrouphead" aria-expanded="${expanded}" data-group="${esc(section.name)}">${expanded ? "▾" : "▸"} <span class="tgname">${esc(section.name)}</span> <span class="tgcount">${counts}</span>${
      section.rationale ? `<span class="tgrationale">${esc(section.rationale)}</span>` : ""
    }</button>
    <div class="tgfiles">${diffFilesHtml(section.files, fileOptions)}</div></div>`;
}

/** The readable files of one changeset as keyed entries, ordered by triage when
 *  a surface plugs the overlay in.
 *
 *  `review` is `{ triage, patch, dial, expandedGroups, overridable }`: the run's
 *  triage payload (or null), the patch those files came from (the hunk ids live
 *  there), whether the reviewer has turned the overlay off, the groups they
 *  have opened, and whether this surface can post their disagreements. Null
 *  `review` — a surface that has no triage to render — takes the plain stack,
 *  one entry per file. */
function reviewStackEntries(files, review, options) {
  const fileEntries = (list, fileOptions) =>
    list.map((file) => ({ key: fileKey(file), html: diffFileHtml(file, fileOptions) }));
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
  const sections = plan.sections.map((section) => {
    if (section.kind === "group")
      return {
        key: `group:${section.name}`,
        html: triageGroupHtml(section, {
          expanded: Boolean(expandedGroups && expandedGroups.has(section.name)),
          fileOptions,
        }),
      };
    const head = section.kind === "critical" ? `<div class="tsectionhead">Needs review first</div>` : "";
    return {
      key: `section:${section.kind}`,
      html: `<div class="tsection t${section.kind}">${head}${diffFilesHtml(section.files, fileOptions)}</div>`,
    };
  });
  return [{ key: "triagebar", html: bar }, ...sections];
}
