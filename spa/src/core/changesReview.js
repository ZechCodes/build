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
import { reviewCommentContext } from "./reviewCommentContext.js";
import { readCached, writeCached } from "./localCache.js";
import { createCommentLayer } from "./changesComments.js";
import { createFileFolds, pathOf } from "./diff.js";
import { diffStackEntries, stackClaims } from "./diffRender.js";
import { DIFF_PLACE_KEEPING, createChangesetPaint } from "./diffPlace.js";
import { changedSinceReview, stampReview } from "./reviewMemory.js";
import { loadTrustDial, saveTrustDial, triageFingerprint } from "./triageModel.js";
import { createTriageOverrides } from "./triageOverride.js";
import { toggleSecretSpoiler } from "./secrets.js";
import { watchChanges } from "./changeEvents.js";
import { paintKeepingPlace } from "./paintKeepingPlace.js";
import { createReviewMarks } from "./reviewMarks.js";
import { watchEditedTimes } from "./editedTime.js";
import { createParsedDiffCache } from "./parsedDiffCache.js";
import { createDiffViewport } from "./diffViewport.js";

export const REVIEW_POLL_MS = 1600;

const fileEditedAtOf = (payload) => payload.file_edited_at || {};

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
 * - `renderIdleActions(actionsHost)` fills the git toolbar's verb host while no
 *   comment is pending; it returns whether it drew anything.
 * - `actionsFrozen()` is the surface's own freeze — an action RPC in flight.
 * - `statusHtml()` is the live claim in the bar (e.g. the agent is working).
 * - `onSent()` runs after comments go out, for whatever the surface does next.
 */
// eslint-disable-next-line complexity -- ratchet: createReviewPlug is at 16, cap 10 — reduce it, then drop this line
export function createReviewPlug({
  fetchDiff,
  // The cache of the machine the diff is on: the surface that made the plug
  // hands it down, and a plug made without one caches nothing.
  cacheScope,
  submit = null,
  submitOverride = null,
  revisionId = () => null,
  renderIdleActions = () => false,
  actionsFrozen = () => false,
  statusHtml = () => "",
  isOffline = () => false,
  pollMs = REVIEW_POLL_MS,
  // A review embedded in a controller that already watches the same checkout
  // can use that controller's refresh path. This avoids a second timer while
  // keeping the plug's serialized, freeze-aware diff fetch intact.
  watchDiff = true,
  // What the diff belongs to — the run or the worktree — so a bridge that
  // pushes can say when it moved instead of being asked every 1.6 seconds. A
  // surface that names none keeps the safety poll and nothing else.
  entity = null,
  cacheEntity = entity,
  navigate = null,
  viewingContext = null,
}) {
  const openFile = (navigate && navigate.openFile) || null;
  let host = null;
  let watcher = null;
  let editedTimeWatcher = null;
  let diffKey = null;
  let responseDiffKey = null;
  const parsedDiffs = createParsedDiffCache();
  let renderedFiles = []; // the freshest parsed diff — what a stamp is taken from
  let renderedPatch = ""; // the patch those files came from — where hunk ids live
  let commentableNow = false;
  let trayMounted = false;
  let noiseExpanded = false; // the collapsed generated-files group at the bottom
  // Review prioritization. `triageReport` is undefined until a payload speaks
  // about triage at all: a surface that never mentions it renders the plain
  // stack, while one that reports `triage: null` has a pass missing and says so.
  let triageReport;
  let triageEnabled = false;
  let triageProject = null;
  let trustDial = false;
  let contextFrame = 0;
  const expandedGroups = new Set(); // the collapsed triage groups the reviewer opened
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
  // when they last sent comments, which files they have approved, which they
  // have selected, and whether the stack is narrowed to only what moved since.
  let reviewStamps = new Map();
  let changedOnlyFilter = false;
  // What the reviewer has marked on these files. The surface hands its own over
  // at mount where it has one, so a mark made on this changeset is the same
  // mark on the stacks beside it.
  let marks = createReviewMarks();

  /** The rendered files as the views a stamp is taken of: a whole patch's file
   *  wears a hash of its own rows as its content key. */
  let fileEditedAt = {};
  const renderedViews = () => renderedFiles;

  const commentLayer = submit
    ? createCommentLayer({
        readNote: () => noteReader(),
        submit: async (messages) => {
          const context = reviewCommentContext({
            paths: renderedFiles.map((file) => file.path), selected: marks.selected,
            mode: "all", snapshot: viewingContext?.snapshot?.(),
          });
          await submit(messages.map((message) => ({ ...message, viewing_context: context })));
          viewingContext?.clearSelectionIfMatches?.(context);
          // Stamp what was just reviewed: the next pass marks what moved.
          reviewStamps = stampReview(renderedViews());
          diffKey = null; // the stamp changes what is drawn — force the rebuild
        },
        revisionId,
        onChange: () => {
          render();
          onCommentsChanged();
        },
      })
    : null;

  /** Redraw the surface's own git verbs, and the tray's, without touching the
   *  diff. The two live in different bars now — merging in the toolbar above
   *  the stack, sending comments in the tray below it — so both are painted. */
  const paintActions = () => {
    if (!host) return;
    if (trayMounted && commentLayer) commentLayer.refreshActions();
    const gitActions = gitActionsHost();
    if (gitActions && !renderIdleActions(gitActions)) gitActions.innerHTML = "";
  };

  let paintChangeset = null;
  const viewport = createDiffViewport({ repaint: render });
  const contextScroller = () => host?.closest(".cdetail-host") || host;
  let contextForce = false;
  const composerFocused = () => Boolean(host?.ownerDocument.activeElement?.closest?.(".composer"));
  const syncViewingContext = () => {
    contextFrame = 0;
    const forced = contextForce;
    contextForce = false;
    if (!forced && composerFocused()) return;
    const scroller = contextScroller();
    if (viewingContext && scroller) viewingContext.setVisibleDiffs(scroller, "all");
  };
  const scheduleViewingContext = (force = false) => {
    contextForce = contextForce || force;
    if (!viewingContext || contextFrame || !host) return;
    const view = host.ownerDocument.defaultView || globalThis;
    const schedule = view.requestAnimationFrame || ((callback) => view.setTimeout(callback, 0));
    contextFrame = schedule(syncViewingContext);
  };
  const onContextScroll = () => scheduleViewingContext(true);
  const selectionTouches = (selection, root) =>
    root && (root.contains(selection.anchorNode) || root.contains(selection.focusNode));
  const collapsedInside = (selection, root) => selection && selection.isCollapsed && root && root.contains(selection.anchorNode);
  const captureViewingSelection = () => {
    if (!viewingContext || !host) return;
    const selection = host.ownerDocument.getSelection?.();
    const root = contextScroller();
    if (selection && !selection.isCollapsed && selectionTouches(selection, root)) viewingContext.captureDomSelection(root);
    else if (collapsedInside(selection, root)) viewingContext.clearSelection();
  };
  const attachViewingContext = () => {
    viewingContext?.clearSelection();
    contextScroller()?.addEventListener("scroll", onContextScroll, true);
    host?.ownerDocument.addEventListener("selectionchange", captureViewingSelection);
  };
  const detachViewingContext = () => {
    const mountedHost = host;
    contextScroller()?.removeEventListener("scroll", onContextScroll, true);
    mountedHost?.ownerDocument.removeEventListener("selectionchange", captureViewingSelection);
    if (!contextFrame || !mountedHost) return;
    const view = mountedHost.ownerDocument.defaultView || globalThis;
    (view.cancelAnimationFrame || view.clearTimeout).call(view, contextFrame);
    contextFrame = 0;
  };
  // Where this surface hosts the plug's git verbs — the git toolbar above the
  // diff. A plug mounted without one (a standalone stack, a test) draws none.
  let gitActionsHost = () => null;
  // What the surface's box under the diff is holding. The box sits below the
  // scroller, outside everything this plug paints, so the note riding out with
  // the anchored comments is read from there rather than kept here.
  let noteReader = () => "";
  // The surface's box under the diff names how much is waiting on its button, so
  // it is told whenever that moves.
  let onCommentsChanged = () => {};
  // A mark is the SURFACE's — its bar names what is selected and its commit
  // narrows to it — so the surface is told whenever one moves in here.
  let onMarksChanged = () => {};

  function render() {
    if (!host) return;
    paintKeepingPlace(host, paintStack, DIFF_PLACE_KEEPING);
  }

  function paintStack() {
    const views = renderedViews();
    const changed = changedSinceReview(reviewStamps, views);
    const filesToRender = changedOnlyFilter ? views.filter((file) => changed.has(file.path)) : views;
    const editable = commentableNow && Boolean(commentLayer);
    trayMounted = editable;
    const entries = diffStackEntries(filesToRender, {
      commentable: editable,
      openable: Boolean(openFile),
      changedSince: changed,
      approved: marks.approved,
      selected: marks.selected,
      selectable: true,
      folds,
      approvable: true,
      ...viewport.renderOptions(),
      noiseExpanded,
      empty: emptyStackText(renderedFiles.length, changedOnlyFilter),
      review:
        !triageEnabled || triageReport === undefined
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
      tray: trayMounted ? commentLayer.trayHtml() : "",
    });
    viewport.attach(host.closest(".cdetail-host") || host);
    if (trayMounted) commentLayer.attach(host);
    paintActions();
    wire();
    // What this plug can take has just been settled by the payload that drew
    // the stack, and the box under the diff is the surface's — it cannot know
    // the plug became commentable unless it is told.
    onCommentsChanged();
    syncViewingContext();
  }

  const claimSecret = (event) => toggleSecretSpoiler(event.target);

  const claimNoiseGroup = (event) => {
    if (!event.target.closest(".noisehead")) return false;
    noiseExpanded = !noiseExpanded;
    render();
    return true;
  };

  const claimTrustDial = (event) => {
    if (!event.target.closest(".tdial")) return false;
    trustDial = !trustDial;
    saveTrustDial(triageProject, trustDial);
    render();
    return true;
  };

  const claimOverride = (event) => Boolean(overrides && overrides.handleClick(event));

  const claimTriageGroup = (event) => {
    const head = event.target.closest(".tgrouphead");
    if (!head) return false;
    const name = head.dataset.group;
    if (expandedGroups.has(name)) expandedGroups.delete(name);
    else expandedGroups.add(name);
    render();
    return true;
  };

  /// The reviewer approving a file, or taking it back. An approved file
  /// collapses, which is what makes the stack shorten as they work down it.
  const claimApprove = (event) => {
    const toggle = event.target.closest(".fapprove");
    if (!toggle) return false;
    marks.toggleApproved(pathOf(toggle.dataset.key));
    render();
    onMarksChanged();
    return true;
  };

  const claims = [
    claimSecret,
    claimApprove,
    claimNoiseGroup,
    claimTrustDial,
    claimOverride,
    claimTriageGroup,
    ...stackClaims({
      comments: () => (trayMounted ? commentLayer : null),
      openFile: () => openFile,
      folds: () => folds,
      approved: () => marks.approved,
      repaint: render,
    }),
  ];

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
      if (!target.classList.contains("fselect-box")) return;
      marks.toggleSelected(pathOf(target.dataset.key));
      render();
      onMarksChanged();
    };
    host.onclick = (event) => {
      const file = event.target.closest?.(".file[data-key]");
      const foldPress = Boolean(
        file &&
        !event.target.closest("button, input, label") &&
        (event.target.closest(".fhead") || file.classList.contains("capped")),
      );
      for (const claim of claims)
        if (claim(event)) {
          if (foldPress) viewport.request(file.dataset.key);
          return;
        }
    };
  }

  // The local cache's slot for this surface's aggregate diff, keyed by the
  // entity the diff belongs to. A surface that names none caches nothing.
  const diffAddress = () =>
    cacheEntity ? cacheScope?.address({ entityId: cacheEntity, kind: "diff" }) || null : null;
  let livePainted = false; // a live payload outranks whatever the cache held
  let refreshHeld = false; // news fetched while an interaction froze repainting

  /** The saved diff, painted whole — comment tray and verbs included, from the
   *  commentability the last live paint recorded. A comment is drafted locally
   *  and every send re-verifies against the bridge, so the cost of a state
   *  that moved while away is one refused send, not a wrong write; the cost of
   *  hiding the chrome was the whole actionbar popping in a round trip late. */
  const applyCachedDiff = (value) => {
    fileEditedAt = fileEditedAtOf(value);
    renderedFiles = parsedDiffs.views(value.patch, { editedAt: fileEditedAt });
    renderedPatch = value.patch || "";
    responseDiffKey = value.diff_key || null;
    commentableNow = value.commentable !== false && Boolean(commentLayer);
    // A saved setting can be older than the run payload already on screen.
    // Keep the report in the record, but only a live read may opt this paint
    // into triage; the instant cached diff therefore always starts in file
    // order and cannot expose a disabled overlay while the bridge is offline.
    triageEnabled = false;
    triageReport = undefined;
    if (!value.projectId || value.projectId === triageProject) return;
    triageProject = value.projectId;
    trustDial = loadTrustDial(triageProject);
  };

  const seedFromCache = async () => {
    const address = diffAddress();
    const record = address ? await readCached(address) : undefined;
    if (!record || !host || livePainted) return;
    applyCachedDiff(record.value);
    render();
  };

  // eslint-disable-next-line complexity -- ratchet: this callback is at 22, cap 10 — reduce it, then drop this line
  const paintOnce = async () => {
    if (!host || isOffline()) return;
    let payload;
    try {
      payload = await fetchDiff(responseDiffKey);
    } catch {
      return; // not readable yet (or a handed-off surface) — the poll retries
    }
    if (!host || !payload) return; // unmounted while the RPC was in flight
    const patchUnchanged = Boolean(payload.unchanged);
    if (patchUnchanged)
      payload = {
        ...payload,
        patch: renderedPatch,
        file_edited_at: payload.file_edited_at || fileEditedAt,
      };
    livePainted = true;
    const nextCommentable = payload.commentable !== false && Boolean(commentLayer);
    // The pass, and the project whose dial governs how it is read. A project
    // the plug has not seen before brings its remembered dial with it.
    const nextTriageEnabled = payload.triageEnabled === true;
    const nextTriage = nextTriageEnabled && Object.hasOwn(payload, "triage") ? payload.triage || null : undefined;
    if (payload.projectId && payload.projectId !== triageProject) {
      triageProject = payload.projectId;
      trustDial = loadTrustDial(triageProject);
    }
    const key = [
      String(payload.key ?? ""),
      String(nextCommentable),
      String(nextTriageEnabled),
      String(trustDial),
      triageFingerprint(overrides ? overrides.apply(nextTriage) : nextTriage),
      String(payload.diff_key ?? payload.revision ?? payload.patch ?? ""),
      JSON.stringify(fileEditedAtOf(payload)),
    ].join("\x01");
    // Freeze while the reviewer is mid-comment or the surface has an action in
    // flight, and skip the rebuild when nothing moved (fold state survives too).
    const busy = actionsFrozen() || Boolean(commentLayer && commentLayer.busy());
    if (host.querySelector(".diffbar") && busy) {
      refreshHeld = true;
      paintActions();
      return;
    }
    responseDiffKey = payload.diff_key || null;
    if (host.querySelector(".diffbar") && key === diffKey) {
      paintActions();
      return;
    }
    fileEditedAt = fileEditedAtOf(payload);
    const contentKeys = Object.fromEntries((payload.files || []).map((file) => [file.path, file.content_key]));
    renderedFiles = patchUnchanged
      ? renderedFiles.map((file) => ({ ...file, editedAt: fileEditedAt[file.path] }))
      : parsedDiffs.views(payload.patch, { contentKeys, editedAt: fileEditedAt });
    renderedPatch = payload.patch || "";
    commentableNow = nextCommentable;
    triageReport = nextTriage;
    triageEnabled = nextTriageEnabled;
    diffKey = key;
    // Only a paint that changed anything rewrites the record — the skip branch
    // above already filtered the every-1.6s sameness out.
    const address = diffAddress();
    if (address)
      writeCached(address, {
        patch: payload.patch,
        commentable: payload.commentable !== false,
        triage: Object.hasOwn(payload, "triage") ? payload.triage || null : null,
        triageEnabled: nextTriageEnabled,
        projectId: payload.projectId || null,
        file_edited_at: fileEditedAt,
        diff_key: responseDiffKey,
      });
    render();
  };

  // Push delivery and the safety timer can land together. Serialize them so a
  // slower old response can never paint after a newer one; remember one extra
  // turn so an invalidation received in flight is still observed.
  let paintFlight = null;
  let repaintRequested = false;
  const paint = () => {
    if (paintFlight) {
      repaintRequested = true;
      return;
    }
    paintFlight = (async () => {
      do {
        repaintRequested = false;
        await paintOnce();
      } while (repaintRequested && host);
    })().finally(() => {
      paintFlight = null;
    });
  };

  /** A pushed invalidation can land while a comment draft or popover freezes
   *  repainting. Keep that news pending and consume it as soon as the owning
   *  surface says the interaction ended; the next safety tick is only backup. */
  const resumeHeldRefresh = () => {
    if (!refreshHeld || !host || actionsFrozen() || Boolean(commentLayer && commentLayer.busy())) return;
    refreshHeld = false;
    paint();
  };

  const startDiffWatcher = () => {
    if (!watchDiff) return null;
    return watchChanges({
      refresh: paint,
      intervalMs: pollMs,
      entity,
      pausesWhileHidden: false,
      // Focus tier: the changeset is the working tree and its status.
      kinds: ["git", "files"],
      mode: "realtime",
    });
  };

  return {
    /** Redraw the actionbar — what a surface calls when its own verbs change
     *  (an action settled, a flash message expired) but the diff has not. */
    refreshActions: paintActions,

    /** Re-read the diff through the same conditional-key and freeze path the
     *  plug's watcher uses. Embedded controllers call this from their existing
     *  checkout watcher instead of mounting a duplicate timer. */
    refreshDiff: paint,

    /** Resume an invalidation held to protect a draft/popover. */
    resumeRefresh: resumeHeldRefresh,

    /** The pending comments (and typed general note) the surface holds. */
    busy: () => Boolean(commentLayer && commentLayer.busy()),

    mount(
      element,
      { gitActions = () => null, readNote = () => "", onComments = () => {}, onMarks = () => {}, reviewMarks = null } = {},
    ) {
      detachViewingContext();
      if (watcher) watcher.dispose(); // a mount over a live one reads twice
      if (editedTimeWatcher) editedTimeWatcher.dispose();
      host = element;
      attachViewingContext();
      gitActionsHost = gitActions;
      noteReader = readNote;
      onCommentsChanged = onComments;
      onMarksChanged = onMarks;
      if (reviewMarks) marks = reviewMarks;
      paintChangeset = createChangesetPaint(host);
      diffKey = null; // a fresh host always needs a first paint
      refreshHeld = false;
      livePainted = false;
      responseDiffKey = null;
      host.innerHTML = '<div class="empty">loading…</div>';
      seedFromCache();
      paint();
      // `pausesWhileHidden: false`: this paint has never been visibility-gated,
      // and an event must not do less than the tick it replaced.
      watcher = startDiffWatcher();
      editedTimeWatcher = watchEditedTimes(host);
    },

    /** What the surface's box under the diff should offer while this plug is
     *  the changeset on screen: whether there is an agent to talk to, and how
     *  much is anchored and waiting. */
    commentOffer: () => ({ commentable: commentableNow && Boolean(commentLayer), pending: commentLayer ? commentLayer.count() : 0 }),

    /** Redraw the stack — what the surface calls when a mark it shares with this
     *  plug was made somewhere else. */
    refresh: render,

    /** Send what is anchored, with the note the box is holding. */
    sendComments: () => (commentLayer ? commentLayer.send() : Promise.resolve()),

    unmount() {
      detachViewingContext();
      if (watcher) watcher.dispose();
      watcher = null;
      if (editedTimeWatcher) editedTimeWatcher.dispose();
      editedTimeWatcher = null;
      if (commentLayer) commentLayer.dispose();
      if (overrides) overrides.dispose();
      parsedDiffs.clear();
      viewport.dispose();
      if (host) {
        host.onclick = null;
        host.onchange = null;
      }
      host = null;
      refreshHeld = false;
      viewingContext?.clear();
      trayMounted = false;
    },
  };
}
