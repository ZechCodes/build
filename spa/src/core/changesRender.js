// Markup builders for the Changes surface. Pure — HTML strings in, no DOM —
// so every one of them is unit-tested by string assertion, and every
// git-derived string (branch, path, subject, body, author, comment text) is
// esc()d on the way out.
//
// The surface these draw is the one the UX Redesign Decisions doc specifies:
// a left rail whose top entry is Uncommitted (carrying +/− counts), the commit
// list under it, and the review aggregate ("All changes") below the list rather
// than pinned above it. Every changeset — uncommitted, one commit, the
// aggregate — renders through the SAME stacked full-file diff (core/diffRender's
// diffStackHtml); these builders only draw what wraps it.

import "../styles/surfaces.css";
import { esc, humanAge } from "./text.js";
import { uncommittedTotals, hasUncommittedChanges } from "./changesModel.js";

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
  const classes = ["crow", commit.ahead_of_base ? "ahead" : "", selected ? "sel" : ""].filter(Boolean).join(" ");
  return `<div class="${classes}" data-hash="${esc(commit.hash)}">
    <span class="csubject">${esc(commit.subject)}</span>
    <span class="cmeta"><span class="chash">${esc(commit.short)}</span> · <span class="cauthor">${esc(commit.author)}</span> · <span class="cage">${esc(humanAge(nowSeconds - (commit.time || 0)))}</span></span></div>`;
}

/** The Changes rail: branch control, Uncommitted (top, with its +/− counts),
 *  the commit list with its paging affordance, and — for a surface that has one
 *  — the review aggregate under the list. `selected` is "uncommitted" |
 *  "review" | a commit hash | null (a clean branch, sitting on the list). */
export function changesRailHtml({
  status,
  log,
  selected,
  review = null,
  branchControlHtml = "",
  nowSeconds = Date.now() / 1000,
}) {
  const rrow = (sel, title, sub) =>
    `<div class="${["rrow", selected === sel ? "sel" : ""].filter(Boolean).join(" ")}" data-sel="${sel}">
      <span class="rtitle">${title}</span><span class="rsub mono">${sub}</span></div>`;
  const totals = uncommittedTotals(status);
  const uncommittedRow = rrow(
    "uncommitted",
    "Uncommitted",
    hasUncommittedChanges(status) ? plusMinusHtml(totals) : "clean",
  );
  const commits = ((log && log.commits) || [])
    .map((c) => commitRowHtml(c, { selected: selected === c.hash, nowSeconds }))
    .join("");
  const reviewRow = review ? rrow("review", "All changes", `vs ${esc(review.base || "main")}`) : "";
  return `<div class="crail">
    ${branchControlHtml ? `<div class="crail-branch">${branchControlHtml}</div>` : ""}
    ${uncommittedRow}
    <div class="rhead">Commits</div>
    ${commits || '<div class="empty">No commits yet.</div>'}
    ${log && log.more ? '<div class="gitmore" role="button" tabindex="0">Load older commits…</div>' : ""}
    ${reviewRow}</div>`;
}

/** The uncommitted changeset's header: what this is, its counts, and the
 *  bridge's two truncation notices when its payloads were capped. */
export function uncommittedHeaderHtml(status) {
  const totals = uncommittedTotals(status);
  const fileCount = totals.files;
  return `<div class="csheader"><span class="cstitle">Uncommitted changes</span>${statSummary({
    files_changed: fileCount,
    insertions: totals.insertions,
    deletions: totals.deletions,
  })}</div>
    ${status && status.files_truncated ? `<div class="ftrunc">file list truncated — ${fileCount} shown; a commit here commits the listed files</div>` : ""}
    ${status && status.truncated ? TRUNCATED_NOTICE : ""}`;
}

/** One commit's header: subject, body, identity line, truncation notice. */
export function commitHeaderHtml(show) {
  const body = (show.body || "").trim();
  return `<div class="csheader commitheader" data-hash="${esc(show.hash)}">
    <div class="csub">${esc(show.subject)}</div>
    ${body ? `<pre class="cbody">${esc(body)}</pre>` : ""}
    <div class="cinfo">${esc(show.short)} · ${esc(show.author)} &lt;${esc(show.email)}&gt; · ${statSummary(show.stat || {})}</div></div>
    ${show.truncated ? TRUNCATED_NOTICE : ""}`;
}

/** The commit box: message + hint + the split-button host. Disclosed only while
 *  uncommitted changes exist (commitBoxVisible decides), and it commits
 *  everything — there is no staged set to assemble. */
export function commitBoxHtml() {
  return `<div class="gitcommit">
    <textarea class="gitmsg" placeholder="Commit message…"></textarea>
    <div class="actionbar"><span class="hint githint"></span><div class="right gitcommit-actions"></div></div>
  </div>`;
}

/** The pending-comment tray: what the reviewer has written but not yet sent,
 *  plus the general-comment box. `generalDraft` is re-applied on every repaint
 *  so a rebuild never loses typed text. */
export function commentTrayHtml(comments, { generalDraft = "" } = {}) {
  const rows = (comments || [])
    .map((c) => {
      const location = c.lnA === 0 && c.lnB === 0 ? "" : c.lnA === c.lnB ? `:${c.lnA}` : `:${c.lnA}-${c.lnB}`;
      return `<div class="pcomment"><span class="pcx" data-id="${esc(c.id)}">×</span>
        <span class="psnip">${esc(c.file)}${esc(location)} · ${esc(String(c.snippet || "").replace(/\s+/g, " ").trim().slice(0, 90))}</span>
        <span class="pctext">${esc(c.comment)}</span></div>`;
    })
    .join("");
  return `<div class="plan-feedback csfeedback"><div class="cslist">${rows}</div>
    <textarea class="csgeneral plan-general" placeholder="Add a general comment about these changes…">${esc(generalDraft)}</textarea></div>
    <div class="actionbar"><span class="hint cshint"></span><div class="right csactions"></div></div>`;
}

/** What the detail pane says when nothing is selected (a clean branch opens on
 *  the commit list) — never a blank pane. */
export function changesetPlaceholderHtml(message) {
  return `<div class="empty csplaceholder">${esc(message)}</div>`;
}
