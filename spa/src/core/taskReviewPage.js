import "../styles/taskReview.css";
import { readCached, subscribeCache } from "./localCache.js";
import { reviewSupportAddress, readReviewSupport, reviewSupportFor, NO_REVIEW_SUPPORT } from "./taskReviewSupport.js";
import { createTaskReviewRepository, reviewAddress } from "./taskReviewCache.js";
import { createReviewCommitsReader, reviewCommitsAddress } from "./taskReviewReads.js";
import { reviewSnapshot, reviewDirectory, reviewHeadHtml, reviewDirectoryHtml, reviewRailHtml, reviewBaseline, reviewComparisonBaseline, reviewSinceCount } from "./taskReviewRender.js";
import { selectionForReview, routeReviewSelection, reviewAnchorSelection, switchReviewSelection } from "./taskReviewSelection.js";
import { mountTaskReviewControls } from "./taskReviewControls.js";
import { mountTaskReviewActions } from "./taskReviewActions.js";
import { mountTaskReviewChanges } from "./taskReviewChanges.js";
import { mountTaskReviewFiles } from "./taskReviewFiles.js";
import { mountTaskReviewCreate } from "./taskReviewCreate.js";
import { mountTaskReviewFeedback, openTaskReviewer } from "./taskReviewFeedback.js";
import { wireDirectoryTabs } from "./workspaceDirectoryTabs.js";
import { readTaskRecord, taskAddress } from "./trackerCache.js";
import { timelineRows } from "./trackerTimeline.js";
import { resolveReviewFileReference } from "./reviewFileReference.js";
import { installReviewLinkNavigation } from "./reviewLinkNavigation.js";
import { refreshFeed } from "./taskFeed.js";

const FRAME = `<header class="task-review-header"><div data-review-head></div><details class="task-review-more" data-review-more><summary>More</summary><div class="task-review-more-body"><div data-review-controls></div><div data-review-git-actions></div><div data-review-task-details></div></div></details></header><aside class="task-review-navigation" aria-label="Review commits"><button type="button" class="task-review-sheet-close btn" data-review-commits-close>Close commits</button><div data-review-directories></div><nav data-review-rail></nav></aside><section class="task-review-centre"><div class="task-review-views" role="group" aria-label="Directory view"><button class="btn" type="button" data-review-view="changes">Changes</button><button class="btn" type="button" data-review-view="files">Files</button><button class="btn task-review-comments-tab" type="button" data-review-view="comments">Comments</button></div><p data-review-newer-snapshot class="warn" role="status" hidden></p><p data-review-error class="warn" role="status" hidden></p><div data-review-content></div></section><div data-review-feedback-host class="task-review-standalone-feedback"></div>`;
const setHtml = (node, html) => { if (node.__reviewHtml !== html) { node.innerHTML = html; node.__reviewHtml = html; } };

/** Saved review content is drawn from cache; the URL owns the reading place. */
export function mountTaskReviewPage(host, options) {
  if (options.dedicated === false) return mountTaskReviewCreate(host, options);
  const { deviceId, projectId, taskId, callRpc } = options;
  const scope = { deviceId, projectId, taskId };
  const repository = createTaskReviewRepository({ ...scope, callRpc, generationOf: options.generationOf });
  let record = null;
  let support = NO_REVIEW_SUPPORT;
  let selection = options.reviewRoute || {};
  let rows = [];
  let commits = null;
  let commitsReader = null;
  let commitsKey = "";
  let disposed = false;
  let selectionGeneration = 0;
  let feedback = null;
  let feedbackKey = "";
  let canonicalized = Boolean(options.commentId);
  let readSerial = 0;
  let cachePaint = Promise.resolve();
  let notice = "";
  let pane = null;
  let paneKey = "";
  let panePlace = "";
  let controls = null;
  let controlsKey = "";
  let gitActions = null;
  let gitActionsKey = "";
  let picker = null;
  host.classList.add("task-review");
  host.innerHTML = FRAME;
  const node = (part) => host.querySelector(`[data-review-${part}]`);
  const review = () => record?.review || null;
  const isPr = () => review()?.mode === "pull_request";
  const controlState = () => isPr() ? "" : review()?.state;
  const snapshot = () => reviewSnapshot(review(), selection.snapshotId);
  const directory = () => reviewDirectory(snapshot(), selection.directoryId);
  const workspaces = () => options.workspaces?.() || [];
  const taskIdentities = () => options.task?.()?.identities;
  const baseline = () => reviewComparisonBaseline(review(), rows, snapshot());
  const identity = () => ({ ...scope, snapshot: snapshot(), directory: directory() });
  const layout = () => host.closest(".review-task-template") || host;

  function choose(changes, transition = {}) {
    if (disposed) return;
    selectionGeneration++;
    const next = switchReviewSelection(selection, changes, transition);
    const resolved = resolveFileReference(next);
    selection = selectionForReview(review(), resolved) || resolved;
    if (!selection) return;
    paint();
    options.markReviewRoute?.({ name: "trackerTask", ...scope, review: routeReviewSelection(selection) });
  }

  function paintHead(saved) {
    setHtml(node("head"), reviewHeadHtml(review(), saved, support, { task: options.task?.(), scope, sinceCount: reviewSinceCount(record, reviewBaseline(review(), rows), directory()) }));
    const select = host.querySelector('[data-review-snapshot]');
    if (select) select.onchange = () => {
      const target = review().snapshots.find((row) => row.id === select.value);
      choose({ snapshotId: target.id, revision: target.number });
    };
    const refresh = host.querySelector('[data-review-refresh]');
    if (refresh) refresh.onclick = () => { void repository.refresh(); void pane?.refresh(); void commitsReader?.refresh(); };
    const assign = host.querySelector('[data-review-reviewer]');
    if (assign) assign.onclick = () => {
      picker = openTaskReviewer({ ...options, task: options.task?.(), snapshot: saved, onAssigned: options.onTaskChanged });
      void options.refreshCatalog?.().then((catalog) => picker?.setCatalog(catalog));
    };
    const open = host.querySelector('[data-review-commits-open]');
    if (open) open.onclick = () => setSheet(true);
  }

  function setSheet(open) {
    host.classList.toggle("task-review-sheet-open", open);
    host.querySelector('[data-review-commits-open]')?.setAttribute("aria-expanded", String(open));
    if (open) host.querySelector('[data-review-range]')?.focus();
    else host.querySelector('[data-review-commits-open]')?.focus();
  }
  function paintControls(saved) {
    const key = JSON.stringify([saved?.id, support, controlState(), review()?.mode]);
    if (controlsKey === key) return controls?.update(review(), isPr() ? record : workspaces());
    controlsKey = key;
    controls?.dispose();
    controls = mountTaskReviewControls(node("controls"), {
      ...scope, snapshot: saved, review: review(), record, repository, support, callRpc, workspaces: workspaces(), keepReadingPlace: options.keepReadingPlace,
      onReclaimed: () => refreshFeed(deviceId),
      onTaskChanged: options.onTaskChanged,
      onSaved: async (verb) => {
        if (verb !== "snapshot") return;
        await repository.refresh();
        await hydrate();
        choose({ snapshotId: review()?.snapshots.at(-1)?.id, revision: review()?.snapshots.at(-1)?.number });
      },
    });
  }

  function paintGitActions(saved) {
    const key = saved ? JSON.stringify([saved.id, support, controlState(), review()?.mode]) : "";
    if (gitActionsKey === key) return gitActions?.update(review(), record);
    gitActionsKey = key;
    gitActions?.dispose();
    gitActions = null;
    node("git-actions").innerHTML = "";
    if (saved) gitActions = mountTaskReviewActions(node("git-actions"), {
      ...scope, snapshot: saved, review: review(), record, repository, support, onTaskChanged: options.onTaskChanged, keepReadingPlace: options.keepReadingPlace,
    });
  }


  function paintFeedback(saved) {
    const allowOpinion = !isPr() || support.pullRequests;
    const key = support.comments && saved ? JSON.stringify([saved.id, allowOpinion]) : "";
    if (key === feedbackKey) return feedback?.update();
    feedbackKey = key;
    feedback?.dispose();
    feedback = null;
    const target = options.feedbackHost || node("feedback-host");
    target.innerHTML = "";
    if (key) feedback = mountTaskReviewFeedback(target, {
      ...scope, snapshot: saved, callRpc, allowOpinion, feed: options.feed, projectKey: options.projectKey,
      keepReadingPlace: options.keepReadingPlace,
      onSent: async () => { await repository.refresh(); await options.onTaskChanged?.(); },
      latestSnapshotId: () => review()?.pull_request?.latest_published_snapshot_id,
    });
  }

  function openFiles(path = "") { choose({ view: "files", path, side: null, line: null }); }

  function paintDirectories(saved, dir) {
    // Directory facts and source switching sit above the commit list.
    const html = saved ? reviewDirectoryHtml(saved, dir, selection.view) : "";
    const template = document.createElement("template");
    template.innerHTML = html;
    template.content.querySelector(".task-review-views")?.remove();
    if (saved?.directories.length < 2) template.content.querySelector(".workspace-dirtabs")?.remove();
    setHtml(node("directories"), template.innerHTML);
    const row = node("directories").querySelector('[role="tablist"]');
    if (row) wireDirectoryTabs(row, (id) => choose({ directoryId: id, view: null, path: "", side: null, line: null }));
  }

  function paintRail() {
    if (disposed) return;
    setHtml(node("rail"), reviewRailHtml({ support, selection, commits, baseline: baseline() }));
    node("rail").querySelectorAll('[data-review-range]').forEach((button) => {
      button.onclick = () => { choose({ mode: button.dataset.reviewRange }); setSheet(false); };
    });
    node("rail").querySelectorAll('[data-review-commit]').forEach((button) => {
      button.onclick = () => { choose({ mode: "commit", commit: button.dataset.reviewCommit }); setSheet(false); };
    });
    const more = node("rail").querySelector('[data-review-more-commits]');
    if (more) more.onclick = () => void commitsReader?.more();
  }

  function paintCommits(saved, dir) {
    const key = saved && dir?.is_git && support.commits ? JSON.stringify([saved.id, dir.id]) : "";
    if (key === commitsKey) return;
    commitsReader?.dispose();
    commitsReader = null;
    commits = null;
    commitsKey = key;
    if (key) commitsReader = createReviewCommitsReader({ ...identity(), callRpc, generationOf: options.generationOf,
      onChange: (value) => { commits = value; paintRail(); paintHead(snapshot()); } });
  }

  function comparison() {
    if (selection.mode === "commit" && support.commit) return { mode: "commit", commit: selection.commit };
    if (selection.mode === "since" && support.since && baseline()) return { mode: "since", from_snapshot_id: baseline().id };
    return { mode: "changes" };
  }

  function mountPane(saved, dir, view) {
    if (!dir || !support.diff) return;
    const content = node("content");
    if (dir.status === "unavailable" && view === "changes") { content.textContent = "Source unavailable"; return; }
    if (view === "changes" && dir.status === "not_git") {
      content.innerHTML = '<p>Not a Git repository</p><button class="btn" data-review-open-files>Open Files</button>';
      content.querySelector('button').onclick = () => openFiles();
      return;
    }
    const params = { ...identity(), workspaceId: review().workspace_id, keepReadingPlace: options.keepReadingPlace, callRpc,
      path: selection.path || "", anchor: selection.anchor, comparison: comparison(), comments: rows, identities: taskIdentities(), autoOpen: true,
      onOpenFile: openFiles, onSelectPath: (path) => {
        if (path !== selection.path) {
          panePlace = JSON.stringify([path, null]);
          choose({ path, side: null, line: null });
        }
      },
      onUnreadableCommit: () => { notice = "This commit is unavailable. Showing all changes for this revision."; choose({ mode: "all" }); },
    };
    pane = view === "files" ? mountTaskReviewFiles(content, params) : mountTaskReviewChanges(content, params);
  }

  function paintPane(saved, dir, view) {
    const key = JSON.stringify([saved?.id, dir?.id, view, support.diff, comparison()]);
    if (key !== paneKey) {
      paneKey = key;
      panePlace = "";
      pane?.dispose();
      pane = null;
      node("content").innerHTML = "";
      mountPane(saved, dir, view);
      // Each pane restores the supplied path/anchor during its own cache read.
      // Starting a second open here can outlive a subsequent tree selection.
      panePlace = JSON.stringify([selection.path, selection.anchor]);
      return;
    }
    pane?.updateComments?.(rows);
    paintPanePlace(view);
  }

  function paintPanePlace(view) {
    const place = JSON.stringify([selection.path, selection.anchor]);
    if (place === panePlace) return;
    panePlace = place;
    if (selection.anchor) void pane?.reveal?.(selection.anchor);
    else if (selection.path) void openSelectedPath(view);
  }

  function openSelectedPath(view) {
    if (view === "files") return pane?.open?.(selection.path);
    return pane?.selectPath?.(selection.path);
  }

  function paint() {
    if (disposed) return;
    if (options.keepReadingPlace) options.keepReadingPlace(paintContent);
    else paintContent();
  }

  function resolveFileReference(target) {
    const result = resolveReviewFileReference(review(), target);
    if (result.missing) notice = "File not found in this revision. Showing all changes.";
    return result.selection;
  }

  function paintContent() {
    host.hidden = !support.get && !review();
    selection = resolveFileReference(selection);
    selection = selectionForReview(review(), selection) || selection;
    normalizeComparison();
    const saved = snapshot();
    const dir = directory();
    const view = selection.view || "changes";
    layout().dataset.reviewView = view;
    paintHead(saved);
    paintControls(saved);
    paintGitActions(saved);
    paintFeedback(saved);
    paintDirectories(saved, dir);
    paintCommits(saved, dir);
    paintRail();
    host.querySelectorAll('[data-review-view]').forEach((button) => {
      button.setAttribute("aria-pressed", String(button.dataset.reviewView === view));
      button.onclick = () => choose({ view: button.dataset.reviewView });
    });
    paintPane(saved, dir, view === "comments" ? "changes" : view);
    paintNotices(saved);
  }

  function normalizeComparison() {
    if (!selection.revision || selection.mode === "all") return;
    const allowed = selection.mode === "commit" ? support.commit : support.since && baseline();
    if (allowed) return;
    notice = "This comparison is unavailable for this revision. Showing all changes.";
    selection = switchReviewSelection(selection, { mode: "all" });
  }

  function paintNotices(saved) {
    const newer = node("newer-snapshot");
    const latest = review()?.snapshots.at(-1);
    newer.hidden = !saved || saved.id === latest?.id;
    newer.textContent = latest ? `Revision ${latest.number} is ready` : "";
    const error = node("error");
    error.textContent = notice || record?.error || "";
    error.hidden = !error.textContent;
  }

  async function hydrate() {
    const serial = ++readSerial;
    const [cached, capabilities, taskRecord] = await Promise.all([readCached(reviewAddress(scope)), readReviewSupport(deviceId), readTaskRecord(deviceId, projectId, taskId)]);
    if (disposed || serial !== readSerial) return;
    record = cached?.value || null;
    support = reviewSupportFor(record?.review, capabilities);
    rows = options.rows?.() || timelineRows(taskRecord?.timeline);
    paint();
    canonicalizeRoute();
  }

  function canonicalizeRoute() {
    if (canonicalized || !selection.revision || !review()) return;
    canonicalized = true;
    const target = routeReviewSelection(selection);
    if (JSON.stringify(target) === JSON.stringify(routeReviewSelection(options.reviewRoute))) return;
    options.markReviewRoute?.({ name: "trackerTask", ...scope, review: target });
  }
  async function anchorSelection(anchor) {
    const saved = review()?.snapshots.find((row) => row.id === anchor.snapshot_id);
    const dir = saved?.directories.find((row) => row.id === anchor.directory_id);
    if (!dir) return null;
    const held = await readCached(reviewCommitsAddress({ ...scope, snapshot: saved, directory: dir }));
    return reviewAnchorSelection(review(), anchor, { support, commits: held?.value });
  }
  const readForPaint = () => { cachePaint = hydrate(); return cachePaint; };
  const unwatchRecord = subscribeCache(reviewAddress(scope), () => void readForPaint());
  const unwatchTask = subscribeCache(taskAddress(deviceId, projectId, taskId), () => void readForPaint());
  const unwatchSupport = subscribeCache(reviewSupportAddress(deviceId), () => { void readForPaint().then(() => repository.refresh()); });
  host.querySelector('[data-review-commits-close]').onclick = () => setSheet(false);
  const onKey = (event) => { if (event.key === "Escape") setSheet(false); };
  host.addEventListener("keydown", onKey);
  const stopReviewLinks = installReviewLinkNavigation(scope, (target) => {
    notice = "";
    choose({ snapshotId: null, path: "", side: null, line: null, commit: null, view: "changes", ...target });
  });
  const ready = readForPaint().then(async () => {
    await Promise.all([controls?.ready, gitActions?.ready, commitsReader?.ready, feedback?.ready]);
    void repository.refresh();
  });
  return {
    ready,
    whenPainted: () => cachePaint,
    feedMoved: paint,
    // Reconnect refreshes comparison failures/latest rails through the shared
    // cache service. A task invalidation only needs fresh review metadata.
    refresh() { void repository.refresh(); },
    async openAnchor(anchor) {
      if (disposed) return false;
      const generation = ++selectionGeneration;
      const target = await anchorSelection(anchor);
      if (disposed || generation !== selectionGeneration) return false;
      if (!target) {
        notice = "Original review context unavailable. The comment remains in the task timeline.";
        paint();
        return false;
      }
      notice = `Original context: Revision ${target.revision} · ${anchor.path} · ${anchor.side} line ${anchor.line}`;
      choose(target);
      await pane?.reveal?.(anchor);
      return true;
    },
    async reply(comment) {
      const view = selection.view;
      const available = comment.anchor && await this.openAnchor(comment.anchor);
      if (disposed) return;
      if (view === "comments" && available) choose({ view });
      feedback?.comment(available ? comment.anchor : null, comment.id);
    },
    dispose() {
      disposed = true;
      selectionGeneration++;
      feedback?.dispose();
      unwatchRecord(); unwatchSupport(); unwatchTask();
      host.removeEventListener("keydown", onKey);
      stopReviewLinks();
      pane?.dispose(); controls?.dispose(); gitActions?.dispose(); commitsReader?.dispose(); picker?.close();
    },
  };
}
