// Saved Git review changes. The immutable snapshot and directory are part of
// every cache address; the project is the registered entity that owns them.
import "../styles/taskReviewChanges.css";
import { parseDiff } from "./diff.js";
import { diffRowsHtml } from "./diffRender.js";
import { createChangesetBodies } from "./changesetBodies.js";
import { esc } from "./text.js";
import { hashFromRoute } from "./router.js";
import { readCached, subscribeCache, writeCached } from "./localCache.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import { langForPath } from "./highlight.js";
import { isDotenvPath } from "./secrets.js";
import { reviewDiffAddress, reviewDiffParams } from "./taskReviewReads.js";
import { reviewMutationRpc } from "./taskReviewSupport.js";
import { inlineReviewRowsHtml, placeReviewComposer, reviewReplyAt } from "./taskReviewInline.js";

const FILE_LIMIT = 1_000;
const MAX_REVEAL_PAGES = 32;
const displayError = (error) => String(error?.message || error || "Source unavailable");

const successfulChanges = (value) => Array.isArray(value?.files) && !value.read_error;
const sinceBaselineOf = (list) => list?.since_baseline;
const oldAnchorMatches = (context) => context.comparisonMode !== "since" || context.sinceBaseline === "base";

function inlineThreads(row, context) {
  const { comments, ...scope } = context;
  const old = row.o > 0 && oldAnchorMatches(context) ? inlineReviewRowsHtml(comments, { ...scope, side: "old", line: row.o }) : "";
  const newer = row.n > 0 ? inlineReviewRowsHtml(comments, { ...scope, side: "new", line: row.n }) : "";
  return old + newer;
}

function updateInlineThreads(host, context) {
  host.querySelectorAll(".task-review-file").forEach((section) => {
    const scroll = section.querySelector(".task-review-diff-scroll");
    if (!scroll) return;
    const top = scroll.scrollTop;
    const left = scroll.scrollLeft;
    scroll.querySelectorAll("[data-review-inline]").forEach((row) => row.remove());
    scroll.querySelectorAll("tr[data-side]").forEach((row) => {
      const lines = { o: Number(row.dataset.oldLine), n: Number(row.dataset.newLine) };
      // The shared inline renderer escapes scalars and sanitizes markdown.
      // nosemgrep: typescript.react.security.audit.react-unsanitized-method.react-unsanitized-method
      row.insertAdjacentHTML("afterend", inlineThreads(lines, { ...context, path: section.dataset.reviewPath }));
    });
    scroll.scrollTop = top;
    scroll.scrollLeft = left;
  });
}

function oldLineAction(row, context) {
  if (!row.o) return "";
  if (!oldAnchorMatches(context)) return String(row.o);
  return `<button type="button" data-review-comment data-side="old" data-line="${row.o}" aria-label="Comment on old line ${row.o}">${row.o}</button>`;
}
function sinceHint(context) {
  if (oldAnchorMatches(context)) return "";
  const href = hashFromRoute({ name: "trackerTask", deviceId: context.deviceId, projectId: context.projectId, taskId: context.taskId,
    review: { revision: context.priorRevision, mode: "all", directoryId: context.directoryId, path: context.path } });
  return `<p class="task-review-note" data-review-since-hint>To comment on previous-head lines, <a class="md-ref" href="${esc(href)}">open the prior revision at this path</a>.</p>`;
}

function lineActions(file, commentable, context) {
  return file.rows.map((row) => {
    const html = diffRowsHtml([row], langForPath(file.path), { maskDotenv: isDotenvPath(file.path) });
    if (row.t === "hunk") return html;
    const threads = inlineThreads(row, context);
    if (!commentable) return html + threads;
    const old = oldLineAction(row, context);
    const newer = row.n > 0 ? `<button type="button" data-review-comment data-side="new" data-line="${row.n}" aria-label="Comment on new line ${row.n}">${row.n}</button>` : "";
    return html.replace(/<td class="ln">[^<]*<\/td><td class="ln">[^<]*<\/td>/, `<td class="ln">${old}</td><td class="ln">${newer}</td>`) + threads;
  }).join("");
}

function fileBodyHtml(file, body, commentable, context) {
  const parsed = parseDiff(body.patch || "").find((entry) => entry.path === file.path);
  if (!parsed) return `<p class="task-review-note">${body.read_error ? esc(body.read_error) : body.loading ? "Loading patch…" : "No text diff for this file."}</p>`;
  const rows = lineActions(parsed, commentable, { ...context, path: file.path });
  const failure = body.read_error ? `<p class="task-review-error" role="alert">${esc(body.read_error)}</p>` : "";
  return `${commentable ? sinceHint({ ...context, path: file.path }) : ""}<div class="task-review-diff-scroll"><table><tbody>${rows}</tbody></table></div>${failure}`;
}

function fileHeaderHtml(file, body, viewed) {
  const path = esc(file.path);
  const status = esc(file.status || "Modified");
  const count = `<span class="task-review-count">+${Number(file.additions ?? 0)} −${Number(file.deletions ?? 0)}</span>`;
  const markLabel = viewed ? "Viewed" : "Mark viewed";
  const mark = `<button type="button" class="btn" data-review-viewed aria-pressed="${viewed}" aria-label="${viewed ? "Mark unread" : markLabel}"${file.content_key ? "" : " disabled"}>${markLabel}</button>`;
  return `<div class="task-review-file-head"><button type="button" class="btn" data-review-expand aria-expanded="${Boolean(body?.open)}">${path}</button><span>${status}</span>${count}${mark}<button type="button" class="btn" data-open-file="${path}" aria-label="Open ${path} in Files">Open in Files</button></div>`;
}

function fileHtml(file, body, viewed, commentable, context) {
  const path = esc(file.path);
  const header = fileHeaderHtml(file, body, viewed);
  if (!body?.open) return `<section class="task-review-file" data-review-path="${path}">${header}</section>`;
  const table = fileBodyHtml(file, body, commentable, context);
  const more = body.pages && !body.pages.complete
    ? `<button type="button" class="btn" data-review-more>Load more lines (${body.pages.end} of ${body.pages.total} bytes)</button>` : "";
  return `<section class="task-review-file" data-review-path="${path}">${header}${table}${more}</section>`;
}

function listingClipped(list) {
  return Boolean(list?.files_truncated) || (list?.files?.length ?? 0) > FILE_LIMIT;
}

function shownFiles(state) {
  const files = (state.list?.files ?? []).slice(0, FILE_LIMIT);
  const clipped = listingClipped(state.list);
  const beyond = state.anchor && clipped && !files.some((file) => file.path === state.anchor.path);
  return { files: beyond ? [...files, { path: state.anchor.path, status: "Modified" }] : files, clipped };
}

function changesHtml(state, commentable, bodyOf) {
  const { files, clipped } = shownFiles(state);
  const limit = clipped ? `<p class="task-review-limit" role="status">Showing the first 1,000 changed files. This review has more files than the listing can show.</p>` : "";
  const error = state.error ? `<p class="task-review-error" role="alert">${esc(state.error)}</p>` : "";
  const empty = emptyMessage(state.list, files);
  const rows = files.map((file) => fileWithMarkHtml(file, state, commentable, bodyOf)).join("");
  const history = historyNotice(state.list);
  return `<div class="task-review-changes">${error}${history}${limit}${empty}<div class="task-review-changes-layout">${changedTreeHtml(files, state.selected)}<div class="task-review-stack">${rows}</div></div></div>`;
}

function fileWithMarkHtml(file, state, commentable, bodyOf) {
  const body = bodyOf(file.path);
  const contentKey = file.content_key ?? body?.content_key;
  const viewed = Boolean(contentKey) && state.viewed.get(file.path) === contentKey;
  return fileHtml({ ...file, content_key: contentKey }, { ...body, open: state.open.has(file.path) }, viewed, commentable, state.context);
}


function changedTreeNodes(files) {
  const root = new Map();
  for (const file of files) {
    let children = root;
    const parts = file.path.split("/");
    for (let index = 0; index < parts.length; index++) {
      const name = parts[index];
      if (!children.has(name)) children.set(name, { name, children: new Map() });
      const node = children.get(name);
      if (index === parts.length - 1) node.file = file;
      children = node.children;
    }
  }
  return root;
}

function changedTreeNodeHtml(node, selected, depth) {
  const label = esc(node.name);
  if (node.file) {
    const file = node.file;
    return `<button type="button" role="treeitem" aria-level="${depth}" data-review-tree-path="${esc(file.path)}" aria-selected="${file.path === selected}" title="${esc(file.path)}"><span>${label}</span><small>+${Number(file.additions || 0)} −${Number(file.deletions || 0)}</small></button>`;
  }
  const rows = [...node.children.values()].map((child) => changedTreeNodeHtml(child, selected, depth + 1)).join("");
  return `<div role="treeitem" aria-level="${depth}" aria-expanded="true" data-review-tree-directory>${label}</div><div role="group">${rows}</div>`;
}

function changedTreeHtml(files, selected) {
  if (!files.length) return "";
  const rows = [...changedTreeNodes(files).values()].map((node) => changedTreeNodeHtml(node, selected, 1)).join("");
  return `<nav class="task-review-changed-tree" role="tree" aria-label="Changed files">${rows}</nav>`;
}

function historyNotice(list) {
  if (!list?.rewritten) return "";
  const comparison = list.since_baseline === "base" ? "Showing the whole revision because the previous head is unavailable." : "Comparing the previous reviewed head with this revision.";
  return `<p class="task-review-limit" role="status">History changed. ${comparison}</p>`;
}

function unreadableCommit(error) {
  return ["commit_outside_range", "commit_unavailable", "source_unavailable"].includes(error?.details?.reason)
    || error?.code === "unavailable" || error?.error_code === "unavailable";
}

function emptyMessage(list, files) {
  if (!list) return `<p class="task-review-note">Loading changes…</p>`;
  if (!files.length) return `<p class="task-review-note">No committed changes in this directory.</p>`;
  return "";
}

function anchoredRow(host, anchor) {
  const section = [...host.querySelectorAll("[data-review-path]")].find((row) => row.dataset.reviewPath === anchor.path);
  const key = anchor.side === "old" ? "oldLine" : "newLine";
  return [...(section?.querySelectorAll("tr[data-side]") || [])].find((row) => Number(row.dataset[key]) === anchor.line) || null;
}

function listedPath(list, path) {
  const files = list?.files || [];
  if (files.some((file) => file.path === path)) return true;
  return Boolean(list?.files_truncated || files.length > FILE_LIMIT);
}

function validAnchor(target) {
  return Boolean(target?.path) && ["old", "new"].includes(target.side) && Number(target.line) > 0;
}

const FOCUS_CONTROLS = ["data-review-expand", "data-review-viewed", "data-review-more", "data-open-file", "data-review-comment"];

function focusedControl(host) {
  const active = host.ownerDocument.activeElement;
  if (!host.contains(active)) return null;
  const path = active.closest("[data-review-path]")?.dataset.reviewPath;
  const control = FOCUS_CONTROLS.find((name) => active.hasAttribute(name));
  return path && control ? { path, control, side: active.dataset.side, line: active.dataset.line } : null;
}

function restoreFocusedControl(host, saved) {
  if (!saved) return;
  const section = [...host.querySelectorAll("[data-review-path]")].find((row) => row.dataset.reviewPath === saved.path);
  const candidates = [...(section?.querySelectorAll(`[${saved.control}]`) || [])];
  const matching = candidates.find((button) => button.dataset.side === saved.side && button.dataset.line === saved.line);
  const fallback = saved.control === "data-review-more" ? section?.querySelector("[data-review-expand]") : null;
  (matching || fallback)?.focus({ preventScroll: true });
}

async function seekAnchorPages(host, target, bodies, current) {
  for (let page = 0; page < MAX_REVEAL_PAGES; page++) {
    if (!current()) return false;
    if (anchoredRow(host, target)) return true;
    const pages = bodies.bodyOf(target.path)?.pages;
    if (!pages || pages.complete || !(await bodies.more(target.path))) return false;
  }
  return current() && Boolean(anchoredRow(host, target));
}

/** Mount one saved source's Changes tab. refresh() retries the listing. */
export function mountTaskReviewChanges(host, { deviceId, projectId, taskId, snapshot, directory, callRpc, onOpenFile, onComment, onReply, onPaint, priorRevision, anchor = null, comparison = {}, path = "", onSelectPath = null, onUnreadableCommit = null, comments = [], identities, autoOpen = false, keepReadingPlace = (paint) => paint() }) {
  const scope = { deviceId, projectId, taskId, snapshot, directory };
  const params = reviewDiffParams(scope, comparison);
  const rpc = reviewMutationRpc(deviceId, callRpc);
  const listAddress = reviewDiffAddress(scope, comparison);
  const patchAddress = (path) => reviewDiffAddress(scope, comparison, path);
  const ui = uiAddress({ deviceId, entityId: projectId, view: "task-review-changes", kind: "review", sub: JSON.stringify([taskId, directory.id]) });
  const state = { list: null, error: "", viewed: new Map(), open: new Set(), loading: new Set(), anchor: null, anchorScrolled: false, selected: path, context: { comments, deviceId, projectId, taskId, priorRevision, identities, snapshotId: snapshot.id, directoryId: directory.id, commit: comparison.commit, comparisonMode: comparison.mode, replyable: Boolean(onReply) } };
  let initialSelection = true;
  let alive = true;
  let generation = 0;
  let selectionGeneration = 0;
  const pendingPatches = new Map();
  const contentKeyOf = (path) => state.list?.files?.find((file) => file.path === path)?.content_key ?? `unlisted:${path}`;
  const bodies = createChangesetBodies({
    addressOf: patchAddress,
    fetchFiles: async (paths, { range } = {}) => {
      const answer = await rpc("tasks.review.diff", { ...params, paths, ...(range ? { range } : {}) });
      if (!alive) throw new Error("Review changed while reading a patch");
      return answer;
    },
    keyFor: contentKeyOf,
    canPage: () => true,
    onChange: () => paint(),
  });

  const paint = () => {
    if (!alive) return;
    keepReadingPlace(() => {
      const focus = focusedControl(host);
      host.innerHTML = changesHtml(state, Boolean(onComment), (path) => ({ ...bodies.bodyOf(path), loading: state.loading.has(path) }));
      restoreFocusedControl(host, focus);
    });
    onPaint?.();
    const row = state.anchor && anchoredRow(host, state.anchor);
    if (!row) return;
    row.classList.add("task-review-anchor");
    if (!state.anchorScrolled) row.scrollIntoView?.({ block: "center" });
    state.anchorScrolled = true;
  };
  const readList = async () => {
    const record = await readCached(listAddress);
    if (!alive) return;
    state.list = record?.value || null;
    state.context.sinceBaseline = sinceBaselineOf(state.list);
    state.error = record?.value?.read_error || "";
    paint();
    if (state.list?.files && initialSelection) await openInitialPath();
    return record;
  };
  const clearPatchError = async (address) => {
    const latest = (await readCached(address))?.value;
    if (!latest?.read_error) return;
    const { read_error: _recoveredError, ...recovered } = latest;
    await writeCached(address, recovered);
  };
  const currentRead = (readGeneration) => alive && readGeneration === generation;
  const validatePatchError = async (path, readGeneration) => {
    const address = patchAddress(path);
    const cached = (await readCached(address))?.value;
    if (!cached?.read_error || !currentRead(readGeneration)) return;
    try {
      const answer = await rpc("tasks.review.diff", { ...params, paths: [path], patch: false });
      if (!currentRead(readGeneration) || !answer.files?.some((file) => file.path === path)) return;
      await clearPatchError(address);
    } catch { /* The cached patch error remains visible until validation succeeds. */ }
  };
  const validateOpenPatchErrors = async (readGeneration) => {
    for (const path of state.open) await validatePatchError(path, readGeneration);
  };
  const listFailure = async (error, readGeneration) => {
    if (!currentRead(readGeneration)) return;
    if (comparison.mode === "commit" && unreadableCommit(error)) onUnreadableCommit?.(error);
    const cached = (await readCached(listAddress))?.value || {};
    await writeCached(listAddress, { ...cached, read_error: displayError(error) });
    await readList();
  };
  const refresh = async () => {
    const readGeneration = ++generation;
    try {
      const answer = await rpc("tasks.review.diff", { ...params, patch: false });
      if (!alive || readGeneration !== generation) return;
      await writeCached(listAddress, answer);
      await readList();
      await validateOpenPatchErrors(readGeneration);
    } catch (error) {
      await listFailure(error, readGeneration);
    }
  };

  const fetchPatch = (path) => {
    if (pendingPatches.has(path)) return pendingPatches.get(path);
    state.loading.add(path);
    paint();
    const pending = (async () => {
      try {
        await bodies.sync([{ path, contentKey: contentKeyOf(path) }], new Set([path]), { budget: 1 });
      } catch (error) {
        if (alive) {
          const cached = (await readCached(patchAddress(path)))?.value || {};
          await writeCached(patchAddress(path), { ...cached, read_error: displayError(error) });
        }
      } finally {
        state.loading.delete(path);
        pendingPatches.delete(path);
        paint();
      }
    })();
    pendingPatches.set(path, pending);
    return pending;
  };

  const uiState = watchUiState(ui, (saved) => {
    if (!alive) return;
    state.viewed = new Map(Object.entries(saved?.viewed || {}));
    paint();
  });
  const persistViewed = () => void uiState.write({ viewed: Object.fromEntries(state.viewed) });
  const viewedContentKey = (path) => state.list?.files?.find((file) => file.path === path)?.content_key ?? bodies.bodyOf(path)?.content_key;
  const toggleViewed = (path) => {
    const contentKey = viewedContentKey(path);
    if (!contentKey) return;
    if (state.viewed.get(path) === contentKey) state.viewed.delete(path);
    else state.viewed.set(path, contentKey);
    persistViewed();
    paint();
  };
  const beginSelection = (name) => {
    initialSelection = false;
    state.selected = name;
    state.anchor = null;
    state.anchorScrolled = false;
    return ++selectionGeneration;
  };
  const currentSelection = (serial) => alive && serial === selectionGeneration;
  const selectPath = async (name, notify = false) => {
    const serial = beginSelection(name);
    if (notify) onSelectPath?.(name);
    if (!name) { paint(); return; }
    state.open.add(name);
    paint();
    await fetchPatch(name);
    if (!currentSelection(serial)) return;
    const section = [...host.querySelectorAll("[data-review-path]")].find((row) => row.dataset.reviewPath === name);
    section?.scrollIntoView?.({ block: "nearest" });
  };
  const clearMissingCommitPath = () => {
    if (comparison.mode !== "commit") return;
    state.selected = "";
    onSelectPath?.("");
  };
  const openInitialPath = async () => {
    initialSelection = false;
    if (path && !listedPath(state.list, path)) {
      clearMissingCommitPath();
      return;
    }
    const selected = path || (autoOpen ? state.list.files[0]?.path : "");
    if (selected) await selectPath(selected);
  };
  const toggleOpen = (path) => {
    beginSelection(path);
    onSelectPath?.(path);
    if (state.open.has(path)) state.open.delete(path);
    else {
      state.open.add(path);
      void fetchPatch(path);
    }
    paint();
  };
  const commentAt = (target, path) => {
    const control = target.closest("[data-review-comment]");
    if (!control || !onComment) return false;
    const line = Number(control.dataset.line);
    if (line > 0) onComment({ snapshot_id: snapshot.id, directory_id: directory.id, path, side: control.dataset.side, line, ...(comparison.commit ? { commit: comparison.commit } : {}) });
    return true;
  };
  const actionHandlers = [
    ["[data-open-file]", (path) => onOpenFile?.(path)],
    ["[data-review-viewed]", toggleViewed],
    ["[data-review-expand]", toggleOpen],
    ["[data-review-more]", (path) => void bodies.more(path)],
  ];
  const onClick = (event) => {
    if (reviewReplyAt(event, state.context.comments, onReply, (row) => ({ snapshot_id: snapshot.id, directory_id: directory.id, path: state.selected, side: row?.dataset.side || "new", line: Number(row?.dataset.newLine), ...(comparison.commit ? { commit: comparison.commit } : {}) }))) return;
    const treePath = event.target.closest("[data-review-tree-path]");
    if (treePath && host.contains(treePath)) { void selectPath(treePath.dataset.reviewTreePath, true); return; }
    const row = event.target.closest("[data-review-path]");
    if (!row || !host.contains(row)) return;
    const path = row.dataset.reviewPath;
    if (commentAt(event.target, path)) return;
    const action = actionHandlers.find(([selector]) => event.target.closest(selector));
    action?.[1](path);
  };
  host.addEventListener("click", onClick);
  const unwatchList = subscribeCache(listAddress, () => void readList());
  const initialRead = readList();
  void initialRead.then((held) => { if (!successfulChanges(held?.value)) return refresh(); });
  const reveal = async (target) => {
    if (!validAnchor(target)) return false;
    const serial = beginSelection(target.path);
    const current = () => currentSelection(serial);
    await initialRead;
    if (!current()) return false;
    if (!state.list) await refresh();
    if (!current()) return false;
    if (!listedPath(state.list, target.path)) { clearMissingCommitPath(); return false; }
    const selectedAnchor = { path: target.path, side: target.side, line: Number(target.line) };
    state.anchor = selectedAnchor;
    state.open.add(target.path);
    paint();
    await fetchPatch(target.path);
    if (!current()) return false;
    await seekAnchorPages(host, selectedAnchor, bodies, current);
    if (!current()) return false;
    paint();
    return Boolean(anchoredRow(host, selectedAnchor));
  };
  if (anchor) void reveal(anchor);
  void uiState.ready;
  return {
    refresh,
    reveal,
    placeComposer: (row, anchor) => placeReviewComposer(host, row, anchor),
    selectPath: (name) => selectPath(name),
    updateComments(rows) {
      if (!alive) return;
      state.context.comments = rows;
      keepReadingPlace(() => updateInlineThreads(host, state.context));
      onPaint?.();
    },
    dispose() {
      alive = false;
      generation += 1;
      selectionGeneration += 1;
      host.removeEventListener("click", onClick);
      unwatchList();
      bodies.dispose();
      uiState.dispose();
    },
  };
}
