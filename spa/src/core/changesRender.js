// Markup builders for the Changes surface. Pure — HTML strings in, no DOM —
// so every one of them is unit-tested by string assertion, and every
// git-derived string (branch, path, subject, body, author, comment text) is
// esc()d on the way out.
//
// The surface these draw is the one the UX Redesign Decisions doc specifies:
// a left rail whose top entry is the review aggregate ("All changes"),
// Uncommitted (carrying +/− counts) directly under it, and the commit list
// below both — a long commit list would otherwise scroll the two rows a
// reviewer reaches for first out of view. Every changeset — the aggregate,
// uncommitted, one commit —
// renders through the SAME stacked full-file diff (core/diffRender's
// diffStackHtml); these builders only draw what wraps it.

import "../styles/surfaces.css";
import { esc, humanAge } from "./text.js";
import { lineRangeSuffix } from "./anchors.js";
import { uncommittedTotals, hasUncommittedChanges } from "./changesModel.js";
import { diffSortHtml } from "./diffSort.js";

const TRUNCATED_NOTICE = '<div class="ftrunc">diff truncated at 1 MiB — the counts above are exact</div>';

/** The +/− pair the rail entry and the changeset headers both speak in. */
export function plusMinusHtml({ insertions, deletions }) {
  return `<span class="pm"><span class="a">+${insertions}</span> <span class="d">−${deletions}</span></span>`;
}

const statSummary = (stat) => {
  const files = Number(stat && stat.files_changed) || 0;
  return `<span class="gitstat">${files} file${files === 1 ? "" : "s"} ${plusMinusHtml({
    insertions: Number(stat && stat.insertions) || 0,
    deletions: Number(stat && stat.deletions) || 0,
  })}</span>`;
};

/** One commit row: subject over short hash · author · relative age. */
export function commitRowHtml(commit, { selected = false, nowSeconds = Date.now() / 1000 } = {}) {
  const classes = ["crow", commit.ahead_of_base ? "ahead" : "", commit.unpushed ? "unpushed" : "", selected ? "sel" : ""]
    .filter(Boolean)
    .join(" ");
  const title = commit.unpushed ? ' title="Not pushed"' : "";
  return `<div class="${classes}" data-hash="${esc(commit.hash)}"${title}>
    <span class="csubject">${esc(commit.subject)}</span>
    <span class="cmeta"><span class="chash">${esc(commit.short)}</span> · <span class="cauthor">${esc(commit.author)}</span> · <span class="cage">${esc(humanAge(nowSeconds - (commit.time || 0)))}</span></span></div>`;
}

/// The Changes rail, row by row: — for a surface that has one — the review
/// aggregate at the top (where the surface opens), Uncommitted under it with
/// its +/− counts, then the "Commits" head and the commit list with its paging
/// affordance. The branch itself is not named here — the nav bar already says
/// it.
/// `selected` is "uncommitted" | "review" | a commit hash | null (a clean
/// branch, sitting on the list).
///
/// Every row, label and affordance is an entry with a name of its own, so the
/// rail is reconciled by name rather than rewritten: a commit that is still
/// there is still the same element after a poll, and the labels between the
/// rows keep their places without being anything special.
// eslint-disable-next-line complexity -- ratchet: changesRailEntries is at 12, cap 10 — reduce it, then drop this line
export function changesRailEntries({ status, log, selected, review = null, nowSeconds = Date.now() / 1000 }) {
  const rrow = (sel, title, sub) =>
    `<div class="${["rrow", selected === sel ? "sel" : ""].filter(Boolean).join(" ")}" data-sel="${sel}">
      <span class="rtitle">${title}</span><span class="rsub mono">${sub}</span></div>`;
  const commits = (log && log.commits) || [];
  const entries = [];
  if (review)
    entries.push({
      key: "review",
      html: rrow("review", "All changes", review.subtitle ? esc(review.subtitle) : `vs ${esc(review.base || "main")}`),
    });
  entries.push({
    key: "uncommitted",
    html: rrow("uncommitted", "Uncommitted", hasUncommittedChanges(status) ? plusMinusHtml(uncommittedTotals(status)) : "clean"),
  });
  entries.push({ key: "commits-head", html: '<div class="rhead">Commits</div>' });
  for (const commit of commits) {
    entries.push({ key: commit.hash, html: commitRowHtml(commit, { selected: selected === commit.hash, nowSeconds }) });
  }
  if (!commits.length) entries.push({ key: "no-commits", html: '<div class="empty">No commits yet.</div>' });
  if (log && log.more) {
    entries.push({ key: "more", html: '<div class="gitmore" role="button" tabindex="0">Load older commits…</div>' });
  }
  return entries;
}

/** The uncommitted changeset's header: its counts and the notice that the
 *  bridge cut its file list short.
 *
 *  Nothing about a capped diff: `git.status` ships shape and no patch, so the
 *  1 MiB cap falls on one file's body and the file draws its own line for it
 *  (core/fileEntries.js). */
export function uncommittedHeaderHtml(status, { sortOrder = "latest" } = {}) {
  const totals = uncommittedTotals(status);
  const fileCount = totals.files;
  return `<div class="csheader">${statSummary({
    files_changed: fileCount,
    insertions: totals.insertions,
    deletions: totals.deletions,
  })}${diffSortHtml(sortOrder)}</div>
    ${status && status.files_truncated ? `<div class="ftrunc">file list truncated — ${fileCount} shown; a commit here commits the listed files</div>` : ""}`;
}

/** Whether a commit's patch is cut short with nothing said of the whole: cut
 *  by the bridge, or kept in pages (#95) that fall short of a whole whose
 *  weight is unknown. Pages whose total is known say how much is drawn at the
 *  end of the stack instead, and are read on from there. */
const patchCutShort = (show) =>
  show.pages ? !show.pages.complete && show.pages.total == null : Boolean(show.truncated);

/** One commit's header: subject, body, identity line, truncation notice. */
export function commitHeaderHtml(show) {
  return commitIdentityHtml(show) + (patchCutShort(show) ? TRUNCATED_NOTICE : "");
}

/** The subject, body and identity line of one commit. */
function commitIdentityHtml(show) {
  const body = (show.body || "").trim();
  return `<div class="csheader commitheader" data-hash="${esc(show.hash)}">
    <div class="csub">${esc(show.subject)}</div>
    ${body ? `<pre class="cbody">${esc(body)}</pre>` : ""}
    <div class="cinfo">${esc(show.short)} · ${esc(show.author)} &lt;${esc(show.email)}&gt; · ${statSummary(show.stat || {})}</div></div>
    `;
}

/** One pending comment, wherever it was written: where it is, what it quotes,
 *  and what was said about it. */
function pendingCommentRowsHtml(comments) {
  return (comments || [])
    .map((c) => {
      const location = lineRangeSuffix(c.lnA, c.lnB);
      return `<div class="pcomment"><span class="pcx" data-id="${esc(c.id)}">×</span>
        <span class="psnip">${esc(c.file)}${esc(location)} · ${esc(String(c.snippet || "").replace(/\s+/g, " ").trim().slice(0, 90))}</span>
        <span class="pctext">${esc(c.comment)}</span></div>`;
    })
    .join("");
}

/** The Changes surface's pending-comment tray: what the reviewer has anchored
 *  and not yet sent, and the one control that discards it.
 *
 *  Nothing to write in. The box under the diff is where a note is written and
 *  where Send lives, so it is on screen no matter how far down the stack the
 *  reviewer has read (core/changesComposer.js) — and a tray with nothing in it
 *  draws nothing at all. */
export function commentTrayHtml(comments) {
  const pending = comments || [];
  if (!pending.length) return "";
  const count = `${pending.length} comment${pending.length === 1 ? "" : "s"} ready to send`;
  return `<div class="plan-feedback csfeedback"><div class="cslist">${pendingCommentRowsHtml(pending)}</div>
    <div class="cstrayfoot"><span class="hint">${count}</span><button class="btn mini cscancel">Clear</button></div></div>`;
}

/** The bar the selection raises: how many files are in hand, approving all of
 *  them, and letting them go.
 *
 *  Drawn only while something is selected — a row of verbs aimed at nothing is
 *  a row of verbs in the way. Committing is not among them: the box under the
 *  diff is where a commit is written, and it narrows itself to the selection
 *  without being told twice. Nor is commenting: a comment is written where every
 *  other comment is written, in that same box. */
export function selectionBarHtml(count) {
  if (!count) return "";
  return `<div class="selbar"><span class="selcount">${count} file${count === 1 ? "" : "s"} selected</span>
    <button class="btn mini selapprove">Approve all</button>
    <button class="btn mini selclear">Clear</button></div>`;
}

/** What the detail pane says when nothing is selected (a clean branch opens on
 *  the commit list) — never a blank pane. */
export function changesetPlaceholderHtml(message) {
  return `<div class="empty csplaceholder">${esc(message)}</div>`;
}
