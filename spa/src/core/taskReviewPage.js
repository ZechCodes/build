import "../styles/taskReview.css";
import { readCached, subscribeCache } from "./localCache.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import { reviewSupportAddress, readReviewSupport, reviewSupportFor, NO_REVIEW_SUPPORT } from "./taskReviewSupport.js";
import { createTaskReviewRepository, reviewAddress } from "./taskReviewCache.js";
import { reviewSnapshot, reviewDirectory, defaultReviewView, reviewHeadHtml, reviewDirectoryHtml } from "./taskReviewRender.js";
import { mountTaskReviewControls } from "./taskReviewControls.js";
import { mountTaskReviewActions } from "./taskReviewActions.js";
import { mountTaskReviewChanges } from "./taskReviewChanges.js";
import { mountTaskReviewFiles } from "./taskReviewFiles.js";
import { mountTaskReviewFeedback, openTaskReviewer } from "./taskReviewFeedback.js";
import { wireDirectoryTabs } from "./workspaceDirectoryTabs.js";
import { refreshFeed } from "./taskFeed.js";

const FRAME = '<div data-review-head></div><p data-review-newer-snapshot class="warn" role="status" hidden></p><div data-review-controls></div><div data-review-git-actions></div><p data-review-error class="warn" role="status" hidden></p><div data-review-directories></div><div data-review-content></div><div data-review-feedback-host></div>';
const setHtml = (node, html) => { if (node.__reviewHtml !== html) { node.innerHTML = html; node.__reviewHtml = html; } };
const visibleRecord = (record) => {
  const { read_order, sync_read_orders, ...visible } = record || {};
  return visible;
};

/** One task's saved review. All server records arrive through cache reads;
 * drafts and selection live separately and survive a metadata refresh. */
export function mountTaskReviewPage(host, options) {
  const { deviceId, projectId, taskId, callRpc } = options;
  const scope = { deviceId, projectId, taskId };
  const repository = createTaskReviewRepository({ ...scope, callRpc, generationOf: options.generationOf });
  let record = null;
  let support = NO_REVIEW_SUPPORT;
  let selection = {};
  let disposed = false;
  let readSerial = 0;
  let cachePaint = Promise.resolve();
  let notice = "";
  let pane = null;
  let paneKey = "";
  let feedback = null;
  let feedbackKey = "";
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
  const state = watchUiState(uiAddress({ deviceId, entityId: projectId, view: "task-review", kind: "selection", sub: taskId }), (saved) => {
    selection = saved || {};
    paint();
  });
  const choose = (changes) => state.write({ ...selection, ...changes });

  function paintHead(saved) {
    setHtml(node("head"), reviewHeadHtml(review(), saved, support));
    const select = host.querySelector('[data-review-snapshot]');
    if (select) select.onchange = () => void choose({ snapshotId: select.value, directoryId: null, view: null, path: "", anchor: null });
    const refresh = host.querySelector('[data-review-refresh]');
    if (refresh) refresh.onclick = () => { void repository.refresh(); void pane?.refresh(); };
    const assign = host.querySelector('[data-review-reviewer]');
    if (assign) assign.onclick = () => {
      picker = openTaskReviewer({ ...options, task: options.task?.(), snapshot: saved, onAssigned: options.onTaskChanged });
      void options.refreshCatalog?.().then((catalog) => picker?.setCatalog(catalog));
    };
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
        await choose({ snapshotId: review()?.snapshots.at(-1)?.id, directoryId: null, view: null, path: "", anchor: null });
      },
    });
  }

  function paintFeedback(saved) {
    const allowOpinion = !isPr() || support.pullRequests;
    const key = support.comments && saved ? JSON.stringify([saved.id, allowOpinion]) : "";
    if (key === feedbackKey) return feedback?.update();
    feedbackKey = key;
    feedback?.dispose();
    feedback = null;
    node("feedback-host").innerHTML = "";
    if (key) feedback = mountTaskReviewFeedback(node("feedback-host"), { ...scope, snapshot: saved, callRpc, onSent: async () => { await repository.refresh(); await options.onTaskChanged?.(); }, keepReadingPlace: options.keepReadingPlace,
      feed: options.feed, projectKey: options.projectKey, allowOpinion, latestSnapshotId: () => review()?.pull_request?.latest_published_snapshot_id });
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

  function openFiles(path = "") {
    return choose({ view: "files", path, anchor: null });
  }

  function paintDirectories(saved, dir, view) {
    setHtml(node("directories"), saved ? reviewDirectoryHtml(saved, dir, view) : "");
    const row = node("directories").querySelector('[role="tablist"]');
    if (row) wireDirectoryTabs(row, (id) => void choose({ directoryId: id, view: null, path: "", anchor: null }));
    host.querySelectorAll('[data-review-view]').forEach((button) => {
      button.onclick = () => void choose({ view: button.dataset.reviewView });
    });
  }

  function mountPane(saved, dir, view) {
    if (!dir || !support.diff) return;
    const content = node("content");
    if (dir.status === "unavailable" && view === "changes") { content.textContent = "Source unavailable"; return; }
    if (view === "changes" && dir.status === "not_git") {
      content.innerHTML = '<p>Not a Git repository</p><button class="btn" data-review-open-files>Open Files</button>';
      content.querySelector('button').onclick = () => void openFiles();
      return;
    }
    const params = {
      ...scope, workspaceId: review().workspace_id, keepReadingPlace: options.keepReadingPlace, snapshot: saved, directory: dir, callRpc,
      path: selection.path || "", anchor: selection.anchor,
      onOpenFile: openFiles,
      onComment: support.comments ? (anchor) => feedback?.comment(anchor) : null,
    };
    pane = view === "files" ? mountTaskReviewFiles(content, params) : mountTaskReviewChanges(content, params);
  }

  function paintPane(saved, dir, view) {
    const key = JSON.stringify([saved?.id, dir?.id, view, support.diff, support.comments]);
    if (key === paneKey) return;
    paneKey = key;
    pane?.dispose();
    pane = null;
    node("content").innerHTML = "";
    mountPane(saved, dir, view);
  }

  function paint() {
    if (disposed) return;
    // A task embeds this pane above its timeline. Independent review cache
    // reads must keep that task's reader still as the pane changes height.
    if (options.keepReadingPlace) options.keepReadingPlace(paintContent);
    else paintContent();
  }

  function paintContent() {
    host.hidden = !support.get && !record?.review;
    const saved = snapshot();
    const dir = directory();
    if (saved) selection.snapshotId = saved.id;
    if (dir) selection.directoryId = dir.id;
    const view = selection.view || defaultReviewView(dir);
    paintHead(saved);
    paintNewerSnapshot(saved);
    paintControls(saved);
    paintGitActions(saved);
    paintFeedback(saved);
    paintDirectories(saved, dir, view);
    paintPane(saved, dir, view);
    const error = node("error");
    error.textContent = notice || record?.error || "";
    error.hidden = !error.textContent;
  }

  function paintNewerSnapshot(saved) {
    const newer = node("newer-snapshot");
    const latest = review()?.pull_request?.latest_published_snapshot_id;
    newer.hidden = !latest || !saved || saved.id === latest;
    const message = "A newer snapshot arrived while you were reviewing. Review the latest snapshot before merging.";
    if (newer.textContent !== message) newer.textContent = message;
  }

  async function hydrate() {
    const serial = ++readSerial;
    const [cached, capabilities] = await Promise.all([readCached(reviewAddress(scope)), readReviewSupport(deviceId)]);
    if (disposed || serial !== readSerial) return;
    const nextRecord = cached?.value || null;
    const nextSupport = reviewSupportFor(nextRecord?.review, capabilities);
    const changed = JSON.stringify([visibleRecord(record), support]) !== JSON.stringify([visibleRecord(nextRecord), nextSupport]);
    record = nextRecord;
    support = nextSupport;
    if (changed) paint();
  }
  const readForPaint = () => { cachePaint = hydrate(); return cachePaint; };
  const unwatchRecord = subscribeCache(reviewAddress(scope), () => void readForPaint());
  const unwatchSupport = subscribeCache(reviewSupportAddress(deviceId), () => { void readForPaint().then(() => repository.refresh()); });
  const ready = Promise.all([readForPaint(), state.ready]).then(async () => {
    paint();
    await Promise.all([controls?.ready, feedback?.ready, gitActions?.ready]);
    void repository.refresh();
  });
  return {
    ready,
    whenPainted: () => cachePaint,
    feedMoved: paint,
    refresh() { void repository.refresh(); void pane?.refresh(); },
    async openAnchor(anchor) {
      const saved = review()?.snapshots.find((row) => row.id === anchor.snapshot_id);
      if (!saved?.directories.some((row) => row.id === anchor.directory_id)) {
        notice = "Original review context unavailable. The comment remains in the task timeline.";
        paint();
        return false;
      }
      notice = `Original context: snapshot ${saved.number} · ${anchor.path} · ${anchor.side} line ${anchor.line}`;
      await choose({ snapshotId: saved.id, directoryId: anchor.directory_id, view: "changes", path: anchor.path, anchor });
      const revealed = await pane?.reveal?.(anchor);
      if (revealed === false && anchor.side === "new") await openFiles(anchor.path);
      host.scrollIntoView?.({ block: "start" });
      return true;
    },
    async reply(comment) {
      const available = comment.anchor && await this.openAnchor(comment.anchor);
      feedback?.comment(available ? comment.anchor : null, comment.id);
    },
    dispose() {
      disposed = true;
      unwatchRecord(); unwatchSupport(); state.dispose();
      pane?.dispose(); controls?.dispose(); gitActions?.dispose(); feedback?.dispose(); picker?.close();
    },
  };
}
