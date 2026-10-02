// Saved Git review changes. The immutable snapshot and directory are part of
// every cache address; the project is the registered entity that owns them.
import "../styles/taskReviewChanges.css";
import { parseDiff } from "./diff.js";
import { diffRowsHtml } from "./diffRender.js";
import { createChangesetBodies } from "./changesetBodies.js";
import { esc } from "./text.js";
import { readCached, subscribeCache, writeCached } from "./localCache.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import { langForPath } from "./highlight.js";
import { isDotenvPath } from "./secrets.js";

const FILE_LIMIT = 1_000;
const MAX_REVEAL_PAGES = 32;
const addressOf = (deviceId, projectId, kind, sub) => ({ deviceId, entityId: projectId, kind, sub: JSON.stringify(sub) });
const baseParams = (taskId, snapshot, directory) => ({ task_id: taskId, snapshot_id: snapshot.id, directory_id: directory.id, mode: "changes" });
const listSub = (taskId, snapshot, directory) => [taskId, snapshot.id, directory.id];
const patchSub = (taskId, snapshot, directory, path) => [...listSub(taskId, snapshot, directory), path, "old..new"];
const displayError = (error) => String(error?.message || error || "Source unavailable");

function lineActions(file) {
  return file.rows.map((row) => {
    const html = diffRowsHtml([row], langForPath(file.path), { maskDotenv: isDotenvPath(file.path) });
    if (row.t === "hunk") return html;
    const old = row.o > 0 ? `<button type="button" data-review-comment data-side="old" data-line="${row.o}" aria-label="Comment on old line ${row.o}">${row.o}</button>` : "";
    const newer = row.n > 0 ? `<button type="button" data-review-comment data-side="new" data-line="${row.n}" aria-label="Comment on new line ${row.n}">${row.n}</button>` : "";
    return html.replace(/<td class="ln">[^<]*<\/td><td class="ln">[^<]*<\/td>/, `<td class="ln">${old}</td><td class="ln">${newer}</td>`);
  }).join("");
}

function fileBodyHtml(file, body, commentable) {
  const parsed = parseDiff(body.patch || "").find((entry) => entry.path === file.path);
  if (!parsed) return `<p class="task-review-note">${body.read_error ? esc(body.read_error) : body.loading ? "Loading patch…" : "No text diff for this file."}</p>`;
  const rows = commentable ? lineActions(parsed) : diffRowsHtml(parsed.rows, langForPath(file.path), { maskDotenv: isDotenvPath(file.path) });
  const failure = body.read_error ? `<p class="task-review-error" role="alert">${esc(body.read_error)}</p>` : "";
  return `<div class="task-review-diff-scroll"><table><tbody>${rows}</tbody></table></div>${failure}`;
}

function fileHeaderHtml(file, body, viewed) {
  const path = esc(file.path);
  const status = esc(file.status || "Modified");
  const count = `<span class="task-review-count">+${Number(file.additions ?? 0)} −${Number(file.deletions ?? 0)}</span>`;
  const markLabel = viewed ? "Viewed" : "Mark viewed";
  const mark = `<button type="button" data-review-viewed aria-pressed="${viewed}" aria-label="${viewed ? "Mark unread" : markLabel}">${markLabel}</button>`;
  return `<div class="task-review-file-head"><button type="button" data-review-expand aria-expanded="${Boolean(body?.open)}">${path}</button><span>${status}</span>${count}${mark}<button type="button" data-open-file="${path}" aria-label="Open ${path} in Files">Open in Files</button></div>`;
}

function fileHtml(file, body, viewed, commentable) {
  const path = esc(file.path);
  const header = fileHeaderHtml(file, body, viewed);
  if (!body?.open) return `<section class="task-review-file" data-review-path="${path}">${header}</section>`;
  const table = fileBodyHtml(file, body, commentable);
  const more = body.pages && !body.pages.complete
    ? `<button type="button" data-review-more>Load more lines (${body.pages.end} of ${body.pages.total} bytes)</button>` : "";
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
  const rows = files.map((file) => fileHtml(file, { ...bodyOf(file.path), open: state.open.has(file.path) }, state.viewed.has(file.path), commentable)).join("");
  return `<div class="task-review-changes">${error}${limit}${empty}${rows}</div>`;
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

async function seekAnchorPages(host, target, bodies) {
  for (let page = 0; page < MAX_REVEAL_PAGES; page++) {
    if (anchoredRow(host, target)) return true;
    const pages = bodies.bodyOf(target.path)?.pages;
    if (!pages || pages.complete || !(await bodies.more(target.path))) return false;
  }
  return Boolean(anchoredRow(host, target));
}

/** Mount one saved source's Changes tab. refresh() retries the listing. */
export function mountTaskReviewChanges(host, { deviceId, projectId, taskId, snapshot, directory, callRpc, onOpenFile, onComment, anchor = null }) {
  const listAddress = addressOf(deviceId, projectId, "task-review-changes", listSub(taskId, snapshot, directory));
  const patchAddress = (path) => addressOf(deviceId, projectId, "task-review-patch", patchSub(taskId, snapshot, directory, path));
  const ui = uiAddress({ deviceId, entityId: projectId, view: "task-review-changes", kind: "review", sub: JSON.stringify(listSub(taskId, snapshot, directory)) });
  const state = { list: null, error: "", viewed: new Set(), open: new Set(), loading: new Set(), anchor: null, anchorScrolled: false };
  let alive = true;
  let generation = 0;
  const pendingPatches = new Map();
  const contentKeyOf = (path) => state.list?.files?.find((file) => file.path === path)?.content_key ?? `unlisted:${path}`;
  const bodies = createChangesetBodies({
    addressOf: patchAddress,
    fetchFiles: async (paths, { range } = {}) => {
      const answer = await callRpc("tasks.review.diff", { ...baseParams(taskId, snapshot, directory), paths, ...(range ? { range } : {}) });
      if (!alive) throw new Error("Review changed while reading a patch");
      return answer;
    },
    keyFor: contentKeyOf,
    canPage: () => true,
    onChange: () => paint(),
  });

  const paint = () => {
    if (!alive) return;
    const focus = focusedControl(host);
    host.innerHTML = changesHtml(state, Boolean(onComment), (path) => ({ ...bodies.bodyOf(path), loading: state.loading.has(path) }));
    restoreFocusedControl(host, focus);
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
    state.error = record?.value?.read_error || "";
    paint();
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
      const answer = await callRpc("tasks.review.diff", { ...baseParams(taskId, snapshot, directory), paths: [path], patch: false });
      if (!currentRead(readGeneration) || !answer.files?.some((file) => file.path === path)) return;
      await clearPatchError(address);
    } catch { /* The cached patch error remains visible until validation succeeds. */ }
  };
  const validateOpenPatchErrors = async (readGeneration) => {
    for (const path of state.open) await validatePatchError(path, readGeneration);
  };
  const refresh = async () => {
    const readGeneration = ++generation;
    try {
      const answer = await callRpc("tasks.review.diff", { ...baseParams(taskId, snapshot, directory), patch: false });
      if (!alive || readGeneration !== generation) return;
      await writeCached(listAddress, answer);
      await readList();
      await validateOpenPatchErrors(readGeneration);
    } catch (error) {
      if (alive && readGeneration === generation) {
        const cached = (await readCached(listAddress))?.value || {};
        await writeCached(listAddress, { ...cached, read_error: displayError(error) });
        await readList();
      }
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
    state.viewed = new Set(saved?.viewed || []);
    paint();
  });
  const persistViewed = () => void uiState.write({ viewed: [...state.viewed] });
  const toggleViewed = (path) => {
    if (state.viewed.has(path)) state.viewed.delete(path);
    else state.viewed.add(path);
    persistViewed();
    paint();
  };
  const toggleOpen = (path) => {
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
    if (line > 0) onComment({ snapshot_id: snapshot.id, directory_id: directory.id, path, side: control.dataset.side, line });
    return true;
  };
  const actionHandlers = [
    ["[data-open-file]", (path) => onOpenFile?.(path)],
    ["[data-review-viewed]", toggleViewed],
    ["[data-review-expand]", toggleOpen],
    ["[data-review-more]", (path) => void bodies.more(path)],
  ];
  const onClick = (event) => {
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
  void initialRead.then(refresh);
  const reveal = async (target) => {
    await initialRead;
    if (!alive || !validAnchor(target)) return false;
    if (!state.list) await refresh();
    if (!listedPath(state.list, target.path)) return false;
    state.anchor = { path: target.path, side: target.side, line: Number(target.line) };
    state.anchorScrolled = false;
    state.open.add(target.path);
    paint();
    await fetchPatch(target.path);
    await seekAnchorPages(host, state.anchor, bodies);
    paint();
    return Boolean(anchoredRow(host, state.anchor));
  };
  if (anchor) void reveal(anchor);
  void uiState.ready;
  return {
    refresh,
    reveal,
    dispose() {
      alive = false;
      generation += 1;
      host.removeEventListener("click", onClick);
      unwatchList();
      bodies.dispose();
      uiState.dispose();
    },
  };
}
