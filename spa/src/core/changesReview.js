// The review plug every surface with an aggregate diff mounts into the Changes
// rail's "All changes" entry — a run's work against its base branch, an
// external worktree's against its merge base.
//
// It is the same changeset the rest of the surface draws: ONE stacked full-file
// diff (core/diffRender), ONE comment layer whose posts are anchored
// conversation messages (core/changesComments → core/notes), ONE actionbar the
// surface fills with its own verbs while nothing is pending. What is left to
// the mounting surface is only what it alone knows: where the diff comes from,
// where comments go, and what finishing the work means here.
//
// The plug owns its host's DOM and a poll with the same freeze discipline as
// the pane around it: a rebuild mid-comment would drop anchors, the open
// popover, and typed text, and a rebuild mid-action would wipe a busy button.

import "../styles/surfaces.css";
import { changesActionbarHtml } from "./changesRender.js";
import { createCommentLayer } from "./changesComments.js";
import { parseDiff } from "./diff.js";
import { diffStackHtml } from "./diffRender.js";
import { changedSinceReview, stampReview } from "./reviewMemory.js";
import { loadTrustDial, saveTrustDial } from "./triageModel.js";
import { toggleSecretSpoiler } from "./secrets.js";

export const REVIEW_POLL_MS = 1600;

/** The bar over the stack: the totals for the WHOLE diff (the changed-only
 *  filter narrows what is drawn, never what is counted), whatever the surface
 *  says about its own state, and — once there is a baseline to compare against —
 *  the offer to see only what moved since the last comments went out. */
export function reviewBarHtml(files, { statusHtml = "", offerChangedOnly = false, changedOnly = false } = {}) {
  const insertions = files.reduce((total, file) => total + file.add, 0);
  const deletions = files.reduce((total, file) => total + file.del, 0);
  const filter = offerChangedOnly
    ? `<label class="changedonly"><input type="checkbox" class="changedonly-box"${changedOnly ? " checked" : ""}/> Only changes since my review</label>`
    : "";
  return `<div class="diffbar"><span>${files.length} files <span style="color:var(--green)">+${insertions}</span> <span style="color:var(--red)">−${deletions}</span></span>${statusHtml}${filter}</div>`;
}

/** What the stack says when the filter has hidden everything, or there is
 *  nothing to show at all. */
export function emptyStackHtml(totalFiles, changedOnly) {
  if (totalFiles && changedOnly) return '<div class="empty">Nothing changed since your review.</div>';
  return '<div class="empty">No file changes yet.</div>';
}

/**
 * createReviewPlug(options) → { mount(host), unmount(), refreshActions() }.
 *
 * - `fetchDiff()` → `{ patch, commentable?, key?, triage?, projectId? }` (or
 *   null to paint nothing). `commentable` is whether this surface can talk to an
 *   agent right now; `key` is whatever else, besides the patch, changes what is
 *   drawn. A surface that is triaged reports `triage` on every payload — the
 *   run's pass, or null when no pass has read this diff yet — and `projectId`,
 *   which the reviewer's trust dial is remembered under. A surface that never
 *   reports `triage` renders the plain stack, with no overlay and no label.
 * - `submit(messages)` sends the anchored comment posts; omitting it makes the
 *   surface read-only.
 * - `revisionId()` names the revision the anchors belong to.
 * - `renderIdleActions(actionsHost, hintHost)` fills the actionbar while no
 *   comment is pending; it returns whether it drew anything.
 * - `actionsFrozen()` is the surface's own freeze — an action RPC in flight.
 * - `statusHtml()` is the live claim in the bar (e.g. the agent is working).
 * - `onSent()` runs after comments go out, for whatever the surface does next.
 */
export function createReviewPlug({
  fetchDiff,
  submit = null,
  revisionId = () => null,
  hint = "Select code, tap a line, or use ✎ to comment. Comments go to the agent.",
  renderIdleActions = () => false,
  actionsFrozen = () => false,
  statusHtml = () => "",
  isOffline = () => false,
  pollMs = REVIEW_POLL_MS,
}) {
  let host = null;
  let timer = null;
  let diffKey = null;
  let renderedFiles = []; // the freshest parsed diff — what a stamp is taken from
  let renderedPatch = ""; // the patch those files came from — where hunk ids live
  let commentableNow = false;
  let trayMounted = false;
  let noiseExpanded = false; // the collapsed generated-files group at the bottom
  // Review prioritization. `triageReport` is undefined until a payload speaks
  // about triage at all: a surface that never mentions it renders the plain
  // stack, while one that reports `triage: null` has a pass missing and says so.
  let triageReport;
  let triageProject = null;
  let trustDial = false;
  const expandedGroups = new Set(); // the collapsed triage groups the reviewer opened

  // Re-review memory, per plug instance (per session): what the reviewer saw
  // when they last sent comments, which files they have ticked off as read, and
  // whether the stack is narrowed to only what moved since.
  let reviewStamps = new Map();
  const viewedFiles = new Set();
  let changedOnlyFilter = false;

  const commentLayer = submit
    ? createCommentLayer({
        submit: async (messages) => {
          await submit(messages);
          // Stamp what was just reviewed: the next pass marks what moved.
          reviewStamps = stampReview(renderedFiles);
          diffKey = null; // the stamp changes what is drawn — force the rebuild
        },
        revisionId,
        hint,
        onChange: () => render(),
        renderIdle: (actions, hintHost) => {
          if (!renderIdleActions(actions, hintHost)) {
            hintHost.textContent = commentableNow ? hint : "";
            actions.innerHTML = "";
          }
          return true;
        },
      })
    : null;

  /** Redraw the actionbar in place, without touching the diff. */
  const paintActions = () => {
    if (!host) return;
    if (trayMounted && commentLayer) {
      commentLayer.refreshActions();
      return;
    }
    const actions = host.querySelector(".csactions");
    const hintHost = host.querySelector(".cshint");
    if (!actions || !hintHost) return;
    if (renderIdleActions(actions, hintHost)) return;
    hintHost.textContent = "";
    actions.innerHTML = "";
  };

  function render() {
    if (!host) return;
    const changed = changedSinceReview(reviewStamps, renderedFiles);
    const filesToRender = changedOnlyFilter ? renderedFiles.filter((file) => changed.has(file.path)) : renderedFiles;
    const editable = commentableNow && Boolean(commentLayer);
    trayMounted = editable;
    const stack = filesToRender.length
      ? diffStackHtml(filesToRender, {
          commentable: editable,
          changedSince: changed,
          viewed: viewedFiles,
          withViewedToggle: editable,
          noiseExpanded,
          review:
            triageReport === undefined
              ? null
              : { triage: triageReport, patch: renderedPatch, dial: trustDial, expandedGroups },
        })
      : emptyStackHtml(renderedFiles.length, changedOnlyFilter);
    host.innerHTML =
      reviewBarHtml(renderedFiles, {
        statusHtml: statusHtml(),
        offerChangedOnly: reviewStamps.size > 0,
        changedOnly: changedOnlyFilter,
      }) +
      stack +
      (trayMounted ? commentLayer.trayHtml() : changesActionbarHtml());
    if (trayMounted) commentLayer.attach(host);
    else paintActions();
    wire();
  }

  /** The filter and the per-file Viewed box. The filter repaints; Viewed folds
   *  the file in place with NO repaint, so the choice survives the poll. */
  function wire() {
    host.onchange = (event) => {
      const target = event.target;
      if (!target.classList) return;
      if (target.classList.contains("changedonly-box")) {
        changedOnlyFilter = target.checked;
        diffKey = null;
        render();
        return;
      }
      if (!target.classList.contains("fviewed-box")) return;
      const path = target.dataset.file;
      const fileElement = target.closest(".file");
      if (target.checked) {
        viewedFiles.add(path);
        if (fileElement) {
          fileElement.classList.add("collapsed");
          fileElement.classList.remove("capped");
        }
        return;
      }
      viewedFiles.delete(path);
      if (fileElement) fileElement.classList.remove("collapsed");
    };
    // Folding belongs to the Changes pane around this plug; what is this plug's
    // own is revealing a masked secret, opening the noise group, and the
    // comment affordances (✎, a line tap, the tray's remove control).
    host.onclick = (event) => {
      if (toggleSecretSpoiler(event.target)) return;
      if (event.target.closest(".noisehead")) {
        noiseExpanded = !noiseExpanded;
        render();
        return;
      }
      // The trust dial and the triage groups: the reviewer's own reading of how
      // much of the pass's reading to take. The dial is remembered per project.
      if (event.target.closest(".tdial")) {
        trustDial = !trustDial;
        saveTrustDial(triageProject, trustDial);
        render();
        return;
      }
      const groupHead = event.target.closest(".tgrouphead");
      if (groupHead) {
        const name = groupHead.dataset.group;
        if (expandedGroups.has(name)) expandedGroups.delete(name);
        else expandedGroups.add(name);
        render();
        return;
      }
      if (trayMounted && commentLayer) commentLayer.handleClick(event);
    };
  }

  const paint = async () => {
    if (!host || isOffline()) return;
    let payload;
    try {
      payload = await fetchDiff();
    } catch {
      return; // not readable yet (or a handed-off surface) — the poll retries
    }
    if (!host || !payload) return; // unmounted while the RPC was in flight
    renderedFiles = parseDiff(payload.patch);
    renderedPatch = payload.patch || "";
    commentableNow = payload.commentable !== false && Boolean(commentLayer);
    // The pass, and the project whose dial governs how it is read. A project
    // the plug has not seen before brings its remembered dial with it.
    if (Object.hasOwn(payload, "triage")) triageReport = payload.triage || null;
    if (payload.projectId && payload.projectId !== triageProject) {
      triageProject = payload.projectId;
      trustDial = loadTrustDial(triageProject);
    }
    const key = [
      String(payload.key ?? ""),
      String(commentableNow),
      String(trustDial),
      JSON.stringify(triageReport ?? null),
      payload.patch,
    ].join("\x01");
    // Freeze while the reviewer is mid-comment or the surface has an action in
    // flight, and skip the rebuild when nothing moved (fold state survives too).
    const busy = actionsFrozen() || Boolean(commentLayer && commentLayer.busy());
    if (host.querySelector(".diffbar") && (key === diffKey || busy)) {
      paintActions();
      return;
    }
    diffKey = key;
    render();
  };

  return {
    /** Redraw the actionbar — what a surface calls when its own verbs change
     *  (an action settled, a flash message expired) but the diff has not. */
    refreshActions: paintActions,

    /** The pending comments (and typed general note) the surface holds. */
    busy: () => Boolean(commentLayer && commentLayer.busy()),

    mount(element) {
      host = element;
      diffKey = null; // a fresh host always needs a first paint
      host.innerHTML = '<div class="empty">loading…</div>';
      paint();
      timer = setInterval(paint, pollMs);
    },

    unmount() {
      if (timer) clearInterval(timer);
      timer = null;
      if (commentLayer) commentLayer.dispose();
      if (host) {
        host.onclick = null;
        host.onchange = null;
      }
      host = null;
      trayMounted = false;
    },
  };
}
