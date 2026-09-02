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
import { cacheDeviceId } from "./cacheScope.js";
import { readCached, writeCached } from "./localCache.js";
import { changesActionbarHtml } from "./changesRender.js";
import { createCommentLayer } from "./changesComments.js";
import { createFileFolds, parseDiff } from "./diff.js";
import { diffStackEntries, pressedFold, pressedOpenFile } from "./diffRender.js";
import { DIFF_PLACE_KEEPING, createChangesetPaint } from "./diffPlace.js";
import { changedSinceReview, stampReview } from "./reviewMemory.js";
import { loadTrustDial, saveTrustDial, triageFingerprint } from "./triageModel.js";
import { createTriageOverrides } from "./triageOverride.js";
import { toggleSecretSpoiler } from "./secrets.js";
import { watchChanges } from "./changeEvents.js";
import { paintKeepingPlace } from "./paintKeepingPlace.js";

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
export function emptyStackText(totalFiles, changedOnly) {
  return totalFiles && changedOnly ? "Nothing changed since your review." : "No file changes yet.";
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
 * - `submitOverride({ hunk_id, direction, note })` sends the reviewer's
 *   disagreement with where the pass put a hunk; omitting it draws no offers.
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
  submitOverride = null,
  revisionId = () => null,
  hint = "Select code, tap a line, or use ✎ to comment. Comments go to the agent.",
  renderIdleActions = () => false,
  actionsFrozen = () => false,
  statusHtml = () => "",
  isOffline = () => false,
  pollMs = REVIEW_POLL_MS,
  // What the diff belongs to — the run or the worktree — so a bridge that
  // pushes can say when it moved instead of being asked every 1.6 seconds. A
  // surface that names none keeps the safety poll and nothing else.
  entity = null,
  // Where the reader goes when they leave the diff for the file itself:
  // `openFile({ path, line })`, the mounting surface's own navigation. A plug
  // mounted without one offers no such control.
  openFile = null,
}) {
  let host = null;
  let watcher = null;
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
  // Which files the reviewer opened and which they shut. State, not a class
  // list: the stack is drawn from it, so a poll that moves the diff leaves
  // every fold where the reviewer put it.
  const folds = createFileFolds();
  // Disagreeing with the pass: applied to the stack on the tap, sent after, and
  // held here only until the pass comes back carrying it.
  const overrides = submitOverride
    ? createTriageOverrides({
        post: submitOverride,
        onChange: () => {
          diffKey = null; // the reading changed under an unchanged patch
          render();
        },
      })
    : null;
  /** The pass as the reviewer's latest word makes it — what is rendered, and
   *  what the poll compares against. */
  const currentTriage = () => (overrides ? overrides.apply(triageReport) : triageReport);

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

  let paintChangeset = null; // made for the host this plug is mounted into

  function render() {
    if (!host) return;
    paintKeepingPlace(host, paintStack, DIFF_PLACE_KEEPING);
  }

  function paintStack() {
    const changed = changedSinceReview(reviewStamps, renderedFiles);
    const filesToRender = changedOnlyFilter ? renderedFiles.filter((file) => changed.has(file.path)) : renderedFiles;
    const editable = commentableNow && Boolean(commentLayer);
    trayMounted = editable;
    const entries = diffStackEntries(filesToRender, {
      commentable: editable,
      openable: Boolean(openFile),
      changedSince: changed,
      viewed: viewedFiles,
      folds,
      withViewedToggle: editable,
      noiseExpanded,
      empty: emptyStackText(renderedFiles.length, changedOnlyFilter),
      review:
        triageReport === undefined
          ? null
          : {
              triage: currentTriage(),
              patch: renderedPatch,
              dial: trustDial,
              expandedGroups,
              overridable: Boolean(overrides),
            },
    });
    paintChangeset({
      bar: reviewBarHtml(renderedFiles, {
        statusHtml: statusHtml(),
        offerChangedOnly: reviewStamps.size > 0,
        changedOnly: changedOnlyFilter,
      }),
      entries,
      tray: trayMounted ? commentLayer.trayHtml() : changesActionbarHtml(),
    });
    if (trayMounted) commentLayer.attach(host);
    else paintActions();
    wire();
  }

  /** The filter and the per-file Viewed box. Both are the reviewer's state and
   *  both repaint from it — the choice survives the poll because the stack is
   *  drawn from what they chose, not from what a press left in the DOM. */
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
      if (target.checked) viewedFiles.add(path);
      else viewedFiles.delete(path);
      render();
    };
    // What this plug answers for: revealing a masked secret, the noise group,
    // the triage overlay's controls, the comment affordances (✎, a line tap,
    // the tray's remove control), and the folds of the stack it draws.
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
      // Disagreeing with where the pass put a hunk, before the comment layer
      // sees the press: the offer sits on a hunk row, and a line tap there
      // would otherwise open a comment on it.
      if (overrides && overrides.handleClick(event)) return;
      const groupHead = event.target.closest(".tgrouphead");
      if (groupHead) {
        const name = groupHead.dataset.group;
        if (expandedGroups.has(name)) expandedGroups.delete(name);
        else expandedGroups.add(name);
        render();
        return;
      }
      // Out of the diff and into the file: claimed before the folds, since the
      // control sits in a capped file's header and the fold would otherwise
      // eat the press as "expand me".
      if (pressedOpenFile(event.target, openFile)) return;
      if (trayMounted && commentLayer && commentLayer.handleClick(event)) return;
      // Folding, which this plug owns because it owns this stack: a press on
      // the filename bar shuts the file, a press on a capped body opens it,
      // and the repaint that follows draws both from the state.
      if (pressedFold(event.target, folds, viewedFiles)) render();
    };
  }

  // The local cache's slot for this surface's aggregate diff, keyed by the
  // entity the diff belongs to. A surface that names none caches nothing.
  const diffAddress = () => {
    const deviceId = cacheDeviceId();
    return deviceId && entity ? { deviceId, entityId: entity, kind: "diff" } : null;
  };
  let livePainted = false; // a live payload outranks whatever the cache held

  /** The saved diff, painted whole — comment tray and verbs included, from the
   *  commentability the last live paint recorded. A comment is drafted locally
   *  and every send re-verifies against the bridge, so the cost of a state
   *  that moved while away is one refused send, not a wrong write; the cost of
   *  hiding the chrome was the whole actionbar popping in a round trip late. */
  const seedFromCache = async () => {
    const address = diffAddress();
    const record = address ? await readCached(address) : undefined;
    if (!record || !host || livePainted) return;
    renderedFiles = parseDiff(record.value.patch);
    renderedPatch = record.value.patch || "";
    commentableNow = record.value.commentable !== false && Boolean(commentLayer);
    triageReport = record.value.triage || null;
    if (record.value.projectId && record.value.projectId !== triageProject) {
      triageProject = record.value.projectId;
      trustDial = loadTrustDial(triageProject);
    }
    render();
  };

  const paint = async () => {
    if (!host || isOffline()) return;
    let payload;
    try {
      payload = await fetchDiff();
    } catch {
      return; // not readable yet (or a handed-off surface) — the poll retries
    }
    if (!host || !payload) return; // unmounted while the RPC was in flight
    livePainted = true;
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
      triageFingerprint(currentTriage()),
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
    // Only a paint that changed anything rewrites the record — the skip branch
    // above already filtered the every-1.6s sameness out.
    const address = diffAddress();
    if (address)
      writeCached(address, {
        patch: payload.patch,
        commentable: payload.commentable !== false,
        triage: Object.hasOwn(payload, "triage") ? payload.triage || null : null,
        projectId: payload.projectId || null,
      });
    render();
  };

  return {
    /** Redraw the actionbar — what a surface calls when its own verbs change
     *  (an action settled, a flash message expired) but the diff has not. */
    refreshActions: paintActions,

    /** The pending comments (and typed general note) the surface holds. */
    busy: () => Boolean(commentLayer && commentLayer.busy()),

    mount(element) {
      if (watcher) watcher.dispose(); // a mount over a live one reads twice
      host = element;
      paintChangeset = createChangesetPaint(host);
      diffKey = null; // a fresh host always needs a first paint
      livePainted = false;
      host.innerHTML = '<div class="empty">loading…</div>';
      seedFromCache();
      paint();
      // `pausesWhileHidden: false`: this paint has never been visibility-gated,
      // and an event must not do less than the tick it replaced.
      watcher = watchChanges({
        refresh: paint,
        intervalMs: pollMs,
        entity,
        pausesWhileHidden: false,
      });
    },

    unmount() {
      if (watcher) watcher.dispose();
      watcher = null;
      if (commentLayer) commentLayer.dispose();
      if (overrides) overrides.dispose();
      if (host) {
        host.onclick = null;
        host.onchange = null;
      }
      host = null;
      trayMounted = false;
    },
  };
}
