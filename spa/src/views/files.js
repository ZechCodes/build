// The Files tab — a worktree explorer shared by all three surfaces (task,
// external worktree, plain folder). Left pane: the checkout's tree, with
// directories expanding in place (core/fileTree.js). Right pane: a strip of
// open-file tabs (core/fileTabs.js) over a per-type preview of the active one.
// Which directories are expanded, which files are open and which one is active
// are UI state, remembered per checkout.
//
// A workspace's Files is one tree over every directory the workspace has, one
// collapsible root per directory (#174, core/fileRoots.js). There a file is
// named by its root and its path together, each root reads and writes its own
// directory's records, and the open-file tabs stand across the roots,
// remembered per workspace.
//
// Both columns read the cache. A directory's listing is its `tree` record, and
// a `files` push rewriting one moves the tree under the reader; a file's body
// is its `file` record, and one the cache holds opens with no round trip. Two
// reads are left on the wire and both write through: `fs.tree` for a directory
// nothing has ever been written for, and `fs.read` for a file nothing holds.
// A file too large for one record is kept as pages beside it (#95), read a
// page at a time by range as the reader scrolls (core/pagedFileView.js).
//
// A tab keeps its unsaved edits while another tab is active: switching away
// holds them in memory (not in any record), closing that tab asks first, and so
// does leaving the tab with any tab holding them.
//
// Over a checkout the sync layer does not walk (a workspace source, a project's
// own directory) nothing keeps those records true but this tab, so there they
// are a seed: painted at once, then read through anyway.
//
// SECURITY: every name/path is escaped. HTML previews render in a
// `sandbox=""` iframe over a `data:` URL (no scripts, no same-origin); SVG and
// images render via `<img src="blob:...">` — never inlined into the DOM. The
// server fences the scope root and every path; this view never sends host paths.

import { esc, pickAFileText } from "../core/text.js";
import { directoryCacheId, syncWalksCheckout } from "../core/directoryScope.js";
import { deleteCached, readCached, recordWriteOf, subscribeCache } from "../core/localCache.js";
import { FILE_RECORD_KIND, cacheFileBody, filePageReader, filePagesReadable } from "../core/cacheLifetime.js";
import {
  mountPagedFile,
  sourceLinesPainter,
  wholeBytesPainter,
  wholeTextPainter,
} from "../core/pagedFileView.js";
import { markdownHtml } from "../core/markdown.js";
import { langForPath } from "../core/highlight.js";
import { decodeBase64Text, mediaPreviewHtml, previewHasSourceToggle, previewModeFor, sourcePreviewHtml } from "../core/filePreview.js";
export { decodeBase64Text, mediaPreviewHtml, previewHasSourceToggle, previewModeFor, sourcePreviewHtml } from "../core/filePreview.js";
import { initPaneDrawer, paneDrawerHtml } from "../core/paneDrawer.js";
import { isDotenvPath, renderDotenvSourceHtml, SPOILER_DOTS } from "../core/secrets.js";
import { confirmAction } from "../core/confirm.js";
import { createFileViewerState, encodeBase64Text, fileBodyReading, fileModeTrayHtml, fileViewerModes, isMediaPath, sameFile } from "../core/fileViewer.js";
import { mountFileEditor } from "../core/fileEditor.js";
import { captureFileSelection } from "../core/fileSelection.js";
import { mountMeasuredHeight } from "../core/measuredInset.js";
import { mountFileTree } from "../core/fileTree.js";
import { locateRooted, mountFileRoots, rootedKey } from "../core/fileRoots.js";
import { mountFileTabs } from "../core/fileTabs.js";
import { attachMediaSource, createMediaBody, releaseMediaSource } from "../core/mediaBlob.js";
import { requestPriorityFields } from "../core/readRequests.js";
import { bridgeCapabilities } from "../core/changeEvents.js";

const FS_READ_MAX_BYTES = 1_048_576;

function bindPreviewMedia(host, file, pages) {
  const element = host.querySelector(".fmedia, .fimg");
  if (element) attachMediaSource(element, pages, file.mime);
}

function releasePreviewMedia(host) {
  host.querySelectorAll(".fmedia, .fimg").forEach(releaseMediaSource);
}

function bindWholeMedia(host, file, mode, sourceOverride, dotenv) {
  if (dotenv || sourceOverride || mode === "toolarge") return;
  if (fileBodyReading(file.mime) === "media" || mode === "svg") {
    bindPreviewMedia(host, file, [file.content_b64 || ""]);
  }
}

/** Whether the source view for `path`/`mode` should render as a masked dotenv
 *  file (secret-value spoilers). Only the source branch (never a rendered
 *  markdown/html/binary preview) masks. */
export function shouldMaskDotenv(path, mode, showSource) {
  return isDotenvPath(path) && (showSource || mode === "source") && mode !== "binary" && mode !== "toolarge";
}

/** Render the preview body HTML for a fs.read response + a source-override flag.
 *  `path` selects the syntax-highlighting grammar for the source branch. */
// eslint-disable-next-line complexity -- ratchet: previewBodyHtml is at 12, cap 10 — reduce it, then drop this line
function previewBodyHtml(path, file, showSource) {
  const mode = previewModeFor(file.mime, file.truncated);
  const truncNotice = file.truncated ? `<div class="ftrunc">truncated at 1 MiB</div>` : "";
  if (showSource || mode === "source") {
    if (mode === "binary" || mode === "toolarge") return sizePlaceholder(mode, file.size);
    return `${sourcePreviewHtml(path, decodeBase64Text(file.content_b64))}${truncNotice}`;
  }
  if (mode === "markdown") return `<div class="plan">${markdownHtml(decodeBase64Text(file.content_b64))}</div>${truncNotice}`;
  if (mode === "html") return `<iframe class="fhtml" sandbox="" src="data:text/html;base64,${file.content_b64}"></iframe>`;
  if (mode === "svg") return '<img class="fimg" alt="">';
  if (mode === "image") return '<img class="fimg" alt="" style="max-width:100%">';
  if (mode === "audio" || mode === "video") return mediaPreviewHtml(mode);
  return sizePlaceholder(mode, file.size);
}

const sizePlaceholder = (mode, size) =>
  `<div class="fbinary">${mode === "toolarge" ? "file too large to preview" : "binary file"} · ${Number(size) || 0} bytes</div>`;

/** Pure: the preview pane's container-less placeholder states. `idle` and
 *  `error` center a quiet block — the message, and an optional second line
 *  saying what would fill the pane; `loading` centers a throbber. */
export function previewPlaceholderHtml(kind, message = "", hint = "") {
  if (kind === "loading") return `<div class="throbber" role="status" aria-label="loading"></div>`;
  const hintLine = hint ? `<p class="fpidle-hint">${esc(hint)}</p>` : "";
  return `<div class="fpidle${kind === "error" ? " fpidle-error" : ""}"><p class="fpidle-msg">${esc(message)}</p>${hintLine}</div>`;
}

/** One checkout: a file is its path, under one root with no name. The root
 *  reads the scope at every use, because an adopted checkout moves it
 *  (`retargetScope`). */
const singleCheckout = (scopeNow) => {
  const root = { id: null, label: "", get scope() { return scopeNow(); } };
  return { roots: [root], locate: (key) => (key ? { root, path: key } : null), keyOf: (_root, path) => path };
};

/** A workspace's directories as roots: a file is its root and its path. */
const rootedCheckout = (roots) => ({
  roots,
  locate: (key) => (key ? locateRooted(roots, key) : null),
  keyOf: (root, path) => rootedKey(root.id, path),
});

/**
 * renderFilesTab(body, { scope, callRpc, cacheScope }) — mount the browser into
 * `body`. `scope` is the plain server-resolved scope object ({task_id} /
 * {project_id[, worktree_id]}) spread into every fs.* call; `callRpc(method,
 * params)` is the app RPC (fs.* ride the app session, not the terminal socket);
 * `cacheScope` is the cache of the machine that checkout is on, handed down by
 * the view, and a mount without one saves nothing. No polling — fetches only on
 * navigation/selection. Returns { dispose() }.
 *
 * A project or workspace passes `roots` ([{ id, label, scope }], its directories in order)
 * instead of one `scope`, and `layoutEntityId`, the cache entity its
 * explorer-wide UI state (open tabs, folded roots) is filed under. `openAt`
 * then names its root too ({ rootId, path, line }), and `onFileOpen(path,
 * rootId)` hears which root the opened file is in.
 */
export function renderFilesTab(body, { scope, roots, layoutEntityId, callRpc, cacheScope = null, openAt = null, onFileOpen = null, viewingContext = null }) {
  let disposed = false;
  // The tree and the preview are the two columns of the shell's two-column
  // primitive, so the browser's outer box measures like every other tab.
  // `#ftree` is the stable column (what the drawer slides, what the tab bar
  // pins to the bottom of); `.ftree-list` is the part the tree repaints —
  // splitting them is what lets a listing repaint the rows without taking the
  // tab bar below them with it. The preview column is the same split: the
  // open-file tabs stand still over `.fpdoc`, the part a file repaints.
  body.innerHTML = `<div class="files pane-split"><div class="ftree pane-list" id="ftree"><div class="ftree-list" role="tree" aria-label="Files"></div></div><div class="fpreview" id="fpreview"><div class="ftabs" role="tablist" aria-label="Open files" hidden></div><div class="fpdoc idle"></div></div>${paneDrawerHtml("files")}</div>`;
  const treeEl = body.querySelector("#ftree");
  const treeListEl = body.querySelector(".ftree-list");
  const tabStripEl = body.querySelector(".ftabs");
  const previewEl = body.querySelector(".fpdoc");
  let stopPreviewHeadMeasurement = () => {};
  const measurePreviewHead = () => {
    stopPreviewHeadMeasurement();
    const head = previewEl.querySelector(".fphead");
    stopPreviewHeadMeasurement = mountMeasuredHeight(head, previewEl, "--file-preview-head");
  };
  // The placeholder states render container-less (no panel box), centered in
  // the preview area; only a loaded file gets the bordered panel back.
  const showPlaceholder = (kind, message, hint) => {
    stopPreviewHeadMeasurement();
    releasePreviewMedia(previewEl);
    previewEl.classList.add("idle");
    previewEl.innerHTML = previewPlaceholderHtml(kind, message, hint);
  };
  // Below the stacking width the tree is behind the drawer's trigger row rather
  // than beside the preview, so the empty state names it instead of pointing at
  // it.
  const showIdle = () => showPlaceholder("idle", "No file open", "Choose a file from the tree to read it here.");
  showIdle();

  // What the files here are named by: a path, or a root and a path. Everything
  // below holds a file by that key and asks here for its root and its path.
  const checkout = roots ? rootedCheckout(roots) : singleCheckout(() => scope);
  const { locate } = checkout;
  const pathOf = (key) => locate(key)?.path ?? key;
  const openKey = openedKey(checkout, openAt);

  let requestedLine = openAt && openAt.line ? { path: openKey, line: openAt.line } : null;
  let sourceOverride = false; // per-selected-file "view source" toggle
  let viewerState = null;
  let editor = null;
  let selectedPath = null;
  let fileRequest = 0;
  // A background tab's path → its viewer state, while it holds edits or a save
  // is out. The draft's own transitions live in createFileViewerState.
  const drafts = new Map();

  // On a narrow viewport the tree is a drawer over the preview. Only opening a
  // file closes it (the tree does that, below): a directory row, or a click
  // that only selects, is still part of choosing one, and closing the drawer
  // under it would put the choosing away mid-choice. Shut, the trigger over it
  // names the file being read — which the preview's own header says, and the
  // preview is behind the drawer.
  const drawer = initPaneDrawer(body.querySelector(".files"), {
    list: treeEl,
    summary: () => (selectedPath ? pathOf(selectedPath) : pickAFileText),
  });

  /** Every open file holding unsaved edits: the active one and any tab
   *  switched away from with edits in it. */
  const dirtyPaths = () => {
    const dirty = new Set([...drafts].filter(([, state]) => state.snapshot().unsaved).map(([path]) => path));
    if (selectedPath && viewerState?.snapshot().unsaved) dirty.add(selectedPath);
    return dirty;
  };

  const onBeforeUnload = (event) => {
    if (!dirtyPaths().size) return;
    event.preventDefault();
    event.returnValue = "";
  };
  window.addEventListener("beforeunload", onBeforeUnload);

  const confirmDiscard = (paths) => confirmAction({
    title: "Discard file edits?",
    intro: `Your unsaved changes to ${[...paths].map(pathOf).join(", ")} will be lost.`,
    confirmLabel: "Discard edits",
    danger: true,
  });

  const discardDirty = async () => {
    const dirty = dirtyPaths();
    return !dirty.size || confirmDiscard(dirty);
  };

  /** Reloading throws away only the shown file's edits, so only they are asked about. */
  const discardShownEdits = async () => !viewerState?.snapshot().unsaved || confirmDiscard([selectedPath]);

  const publishFileContext = () => {
    if (!viewingContext || !selectedPath) return;
    viewingContext.set({ version: 1, items: [{ kind: "file", path: pathOf(selectedPath) }] });
  };

  const publishContextSelection = (items) => {
    if (!viewingContext) return;
    if (viewingContext.setSelection) viewingContext.setSelection(items);
    else viewingContext.set({ version: 1, items: [{ kind: "file", path: pathOf(selectedPath) }, ...items] });
  };

  const publishEditorSelection = () => {
    const snapshot = viewerState?.snapshot();
    const items = [];
    if (snapshot?.mode === "edit" && snapshot.selection.end > snapshot.selection.start) {
      items.push({
        kind: "selection",
        path: pathOf(selectedPath),
        text: snapshot.value.slice(snapshot.selection.start, snapshot.selection.end),
        ...(snapshot.unsaved ? { unsaved: true } : {}),
      });
    }
    publishContextSelection(items);
  };

  const composerHasFocus = () => Boolean(document.activeElement?.closest?.(".thread-composer"));
  const selectionBelongsToReadingLayer = (selection, readingLayer) =>
    !selection?.anchorNode || readingLayer?.contains(selection.anchorNode);

  const onDocumentSelectionChange = () => {
    if (!selectedPath || viewerState?.snapshot().mode === "edit") return;
    const readingLayer = previewEl.querySelector(".file-reading-layer");
    const selection = document.getSelection();
    const items = captureFileSelection(readingLayer, pathOf(selectedPath), selection);
    if (items.length) {
      publishContextSelection(items);
      return;
    }
    if (!composerHasFocus() && selectionBelongsToReadingLayer(selection, readingLayer)) publishContextSelection([]);
  };
  document.addEventListener("selectionchange", onDocumentSelectionChange);

  // The local cache's address for one of a root's records. A project-scoped
  // checkout names no entity and takes no part.
  const rootAddress = (root, kind, sub) => {
    const entityId = root.cacheEntityId || directoryCacheId(root.scope);
    return entityId ? cacheScope?.address({ entityId, kind, sub }) || null : null;
  };

  /** Whether this tab is the only reader of a root's checkout. A run's or an
   *  external worktree's records are kept true by the sync layer, so what they
   *  hold is the answer. A workspace source's and a project's are not walked by
   *  anybody, so what they hold is the last visit's own work: a seed to paint
   *  at once, and never a reason to skip the read. */
  const rootReadsForItself = (root) => !syncWalksCheckout(root.scope);
  const readsForItself = (key) => rootReadsForItself(locate(key).root);
  const scopeOf = (key) => locate(key).root.scope;

  const fileAddress = (key) => {
    const at = locate(key);
    return at ? rootAddress(at.root, FILE_RECORD_KIND, at.path) : null;
  };

  const heldRecord = (address) => (address ? readCached(address) : Promise.resolve(undefined));

  /** Whether `address` still holds the write `written` names (recordWriteOf),
   *  asked after a wire call: its time is not which write it is. */
  const recordStill = (address, written) => async () => recordWriteOf(await heldRecord(address)) === written;

  let unwatchFile = null;

  const stillSelected = (request, path) =>
    !disposed && request === fileRequest && selectedPath === path;

  // Where the reader was in a paged file being painted again (a new version
  // of it), carried over until the pages painted reach that far again.
  let carriedScroll = null;

  const adoptFile = (path, file) => {
    editor?.dispose();
    editor = null;
    if (file.paged && viewerState?.snapshot().file.paged) {
      carriedScroll = previewEl.querySelector(".file-reading-layer")?.scrollTop || null;
    }
    renderPreview(path, file);
  };

  const restoreCarriedScroll = (scroller) => {
    if (!carriedScroll) return;
    scroller.scrollTop = carriedScroll;
    if (scroller.scrollTop >= carriedScroll) carriedScroll = null;
  };

  /** A record of the selected file arrived: the draft decides whether it is
   *  the same file, a change on disk its edits are kept over, or a fresh
   *  read to show. */
  const takeSelectedFile = (path, request, file) => {
    if (!file || !stillSelected(request, path)) return false;
    if (!viewerState) {
      renderPreview(path, file);
      return true;
    }
    if (viewerState.recordArrived(file) === "adopt") adoptFile(path, file);
    else paintEditStatus();
    return true;
  };

  const rereadSelectedFile = async (path, request) => {
    const record = await heldRecord(fileAddress(path));
    if (record?.value?.file) takeSelectedFile(path, request, record.value.file);
    return record;
  };

  const watchFile = (path, request) => {
    unwatchFile?.();
    unwatchFile = null;
    const address = fileAddress(path);
    if (!address) return;
    unwatchFile = subscribeCache(address, () => void rereadSelectedFile(path, request));
  };

  /** The reader of one page of `path` by range (#95). It asks at each read
   *  whether the bridge can page, so the view holding it never does. */
  const pageReader = (path, file) =>
    filePageReader(cacheScope?.deviceId, (range) => {
      const params = { ...scopeOf(path), path: pathOf(path), range };
      return fileBodyReading(file?.mime) === "media"
        ? callRpc("fs.read", params, requestPriorityFields("background"))
        : callRpc("fs.read", params);
    }, file?.mime);

  /** What a store decides by, after an answer (so after a greeting): the page
   *  reader where this bridge can page, or null where it cannot. */
  const storePageReader = (path, file) => (filePagesReadable(cacheScope?.deviceId) ? pageReader(path, file) : null);

  /** Store one pulled body — whole, or as pages when it is too large for one
   *  record — and then read the stored record back: the view paints what the
   *  cache holds, never the answer. A mount handed no cache has nowhere to
   *  store, so it shows the answer as it came. */
  const storePulledFile = async (path, file, request, previousWrite) => {
    const address = fileAddress(path);
    if (!address) return { file };
    const current = await heldRecord(address);
    if (!stillSelected(request, path)) return {};
    if (recordWriteOf(current) !== previousWrite) return { file: current?.value?.file };
    await cacheFileBody({
      deviceId: address.deviceId, entityId: address.entityId, path: address.sub, file,
      readPage: storePageReader(path, file), still: recordStill(address, previousWrite), written: previousWrite,
    });
    if (!stillSelected(request, path)) return {};
    return { file: (await heldRecord(address))?.value?.file };
  };

  // A paged file is started over from its first page at most once off each
  // version, and never off the version its own start-over wrote: the next
  // start goes with a record somebody else wrote (a push's refresh), so a
  // file written to all the time cannot spin the view. `path\nof` keys.
  let restartedFrom = null;
  let restartedTo = null;

  /** Read a paged file again from its first page: its next page was of a
   *  changed file, or none of its pages are held any more. The new record's
   *  announcement repaints. */
  const restartPagedFile = async (path, file) => {
    const address = fileAddress(path);
    const key = `${path}\n${file.of}`;
    const readPage = storePageReader(path, file);
    if (!address || !readPage || key === restartedFrom || key === restartedTo) return;
    restartedFrom = key;
    const written = recordWriteOf(await heldRecord(address));
    const stored = await cacheFileBody({
      deviceId: address.deviceId, entityId: address.entityId, path: address.sub, file, readPage, still: recordStill(address, written), written,
    });
    if (stored) restartedTo = `${path}\n${(await heldRecord(address))?.value?.file?.of}`;
  };

  const pullFile = async (path, request, previousWrite) => {
    try {
      const raw = bridgeCapabilities(cacheScope?.deviceId)?.bodies?.mediaRawPages === true && isMediaPath(pathOf(path));
      const params = {
        ...scopeOf(path), path: pathOf(path),
        ...(raw ? { range: { offset: 0, bytes: FS_READ_MAX_BYTES, raw: true } } : {}),
      };
      const file = raw
        ? await callRpc("fs.read", params, requestPriorityFields("background"))
        : await callRpc("fs.read", params);
      return storePulledFile(path, file, request, previousWrite);
    } catch (error) {
      return { error };
    }
  };

  const setText = (element, value) => {
    if (element) element.textContent = value;
  };

  /** A save came back, to whichever tab is shown by now: the tab strip
   *  always hears of it; the preview only when it is this file's. A draft
   *  left clean under a newer record reads that record. */
  const afterSave = (path, state, written) => {
    tabs.refresh();
    if (disposed || viewerState !== state) return;
    const stale = state.snapshot().stale;
    if (stale) return adoptFile(path, stale);
    if (written) {
      setText(previewEl.querySelector(".fpsize"), `${Number(written.size) || 0} bytes`);
      setText(treeListEl.querySelector(".frow.sel .fsize"), String(Number(written.size) || 0));
    }
    paintEditStatus();
    publishEditorSelection();
  };

  const beginFileSelection = (path) => {
    if (requestedLine?.path !== path) requestedLine = null;
    onFileOpen?.(pathOf(path), locate(path).root.id);
    tree.setOpenPath(path);
    sourceOverride = false;
    editor?.dispose();
    editor = null;
    viewerState = null;
    selectedPath = path;
    drawer.refresh();
    viewingContext?.clearSelection?.();
    publishFileContext();
    stopPagedView();
    carriedScroll = null;
    showPlaceholder("loading");
  };

  const editorIsCurrent = (path, state) =>
    !disposed && selectedPath === path && viewerState === state;

  const showFileResult = (path, request, result, errorPrefix = "cannot read") => {
    if (!stillSelected(request, path)) return;
    if (result.error) {
      if (!viewerState) showPlaceholder("error", `${errorPrefix}: ${result.error.message || "error"}`);
      return;
    }
    if (result.file) takeSelectedFile(path, request, result.file);
  };

  const takeHeldFile = (path, request, address, record) => {
    const held = record?.value?.file;
    if (!held) return false;
    takeSelectedFile(path, request, held);
    if (readsForItself(path)) return false;
    // Stored again only while the record is still the one shown: a push that
    // landed since the read is newer, and this copy must not write over it.
    if (address) {
      void cacheFileBody({
        deviceId: address.deviceId, entityId: address.entityId, path: address.sub, file: held,
        still: recordStill(address, recordWriteOf(record)), written: recordWriteOf(record),
      });
    }
    return true;
  };

  const selectFile = async (path) => {
    if (disposed || path === selectedPath) return;
    const request = ++fileRequest;
    // The tab names the file it is standing in, so the URL can say so too.
    beginFileSelection(path);
    watchFile(path, request);
    const address = fileAddress(path);
    const record = await heldRecord(address);
    if (!stillSelected(request, path)) return;
    if (takeHeldFile(path, request, address, record)) return;
    const result = await pullFile(path, request, recordWriteOf(record));
    showFileResult(path, request, result);
  };

  // Reveal/hide is EPHEMERAL: a fresh renderPreview re-derives the secrets and
  // starts fully masked, so any repaint/navigation resets to the hidden state.
  const wireDotenvSpoilers = (secrets) => {
    const spoilers = [...previewEl.querySelectorAll(".spoiler[data-secret-index]")];
    // The real values live in `secrets` (JS state) — never in the DOM — until a
    // reveal swaps one in via textContent (never innerHTML, so no markup runs).
    const setSpoiler = (span, reveal) => {
      span.textContent = reveal ? secrets[+span.dataset.secretIndex] : SPOILER_DOTS;
      span.classList.toggle("on", reveal);
    };
    spoilers.forEach((span) => (span.onclick = () => setSpoiler(span, !span.classList.contains("on"))));
    const revealAll = previewEl.querySelector("#fpreveal");
    if (revealAll) {
      let shown = false;
      revealAll.onclick = () => {
        shown = !shown;
        spoilers.forEach((span) => setSpoiler(span, shown));
        revealAll.textContent = shown ? "Hide all" : "Reveal all";
      };
    }
  };

  /** The draft takes the answer as soon as it comes, whichever tab is shown;
   *  the record is written through after, and its push is the same file. */
  const saveEditor = async (path) => {
    const state = viewerState;
    const baseline = state.snapshot().file;
    const write = state.submit();
    if (!write) return;
    paintEditStatus();
    const address = fileAddress(path);
    // The record as the save leaves, read beside the write rather than before it.
    const before = heldRecord(address);
    let written;
    try {
      written = await callRpc("fs.write", {
        ...scopeOf(path),
        path: pathOf(path),
        content_b64: encodeBase64Text(write.value),
        expected_revision: write.revision,
      });
    } catch (error) {
      state.saveFailed(error);
      return afterSave(path, state, null);
    }
    state.saveSucceeded(written);
    afterSave(path, state, written);
    await storeWrittenFile(path, address, written, baseline, recordWriteOf(await before));
  };

  /** A cache access refresh changes the timestamp without changing the file.
   *  Replace the submitted baseline, but keep a competing revision visible.
   *  If the record disappeared while saving, preserve that invalidation too. */
  const storeWrittenFile = async (path, address, written, baseline, previousWrite) => {
    if (!address) return false;
    const current = await heldRecord(address);
    if (current?.value?.file) {
      if (!sameFile(current.value.file, baseline)) return false;
    } else if (recordWriteOf(current) !== previousWrite) return false;
    const kept = await cacheFileBody({ deviceId: address.deviceId, entityId: address.entityId, path: address.sub, file: written });
    if (!kept) await deleteCached([address]);
    return kept;
  };

  const showReloadResult = (path, request, state, previousValue, result) => {
    if (!stillSelected(request, path) || viewerState !== state) return;
    if (state.snapshot().value !== previousValue) return;
    if (result.error) {
      previewEl.querySelector(".file-save-status").textContent = result.error.message || "Reload failed";
      return;
    }
    editor?.dispose();
    editor = null;
    if (!result.file) return;
    renderPreview(path, result.file);
  };

  const reloadEditor = async (path) => {
    const expectedState = viewerState;
    if (!await discardShownEdits()) return;
    if (!editorIsCurrent(path, expectedState)) return;
    const reloadingState = viewerState;
    const reloadingValue = reloadingState.snapshot().value;
    const request = ++fileRequest;
    watchFile(path, request);
    const before = await heldRecord(fileAddress(path));
    const result = await pullFile(path, request, recordWriteOf(before));
    showReloadResult(path, request, reloadingState, reloadingValue, result);
  };

  const paintEditor = (path, snapshot) => {
    const bodyHost = previewEl.querySelector(".file-editor-layer");
    editor = mountFileEditor(bodyHost, {
      value: snapshot.value,
      selection: snapshot.selection,
      onEdit: (value, selection) => {
        viewerState.edit(value, selection);
        paintEditStatus();
        publishEditorSelection();
      },
      onSelection: (selection) => {
        viewerState.edit(viewerState.snapshot().value, selection);
        publishEditorSelection();
      },
    });
    previewEl.querySelector(".file-save").onclick = () => saveEditor(path);
    previewEl.querySelector(".file-reload").onclick = () => reloadEditor(path);
    paintEditStatus();
  };

  const statusText = (snapshot) => (snapshot.disk ? "File changed on disk" : snapshot.error || "");

  /** Paint the shown draft's state: Unsaved while it holds anything not on
   *  disk, Save only when there are edits and no save is out, the disk change
   *  or the refusal, and Reload whenever the file on disk is not the baseline. */
  const paintEditStatus = () => {
    const snapshot = viewerState.snapshot();
    const dirty = previewEl.querySelector(".file-dirty");
    const save = previewEl.querySelector(".file-save");
    const reload = previewEl.querySelector(".file-reload");
    if (dirty) dirty.hidden = !snapshot.unsaved;
    if (save) save.disabled = snapshot.status !== "dirty";
    setText(previewEl.querySelector(".file-save-status"), statusText(snapshot));
    if (reload) reload.hidden = !(snapshot.disk || snapshot.conflict);
    tabs.refresh();
  };

  const previewFile = (snapshot) => snapshot.modes.length && !snapshot.file.paged ? ({
      ...snapshot.file,
      content_b64: encodeBase64Text(snapshot.value),
      size: new TextEncoder().encode(snapshot.value).length,
    }) : snapshot.file;

  let pagedView = null;
  const stopPagedView = () => {
    releasePreviewMedia(previewEl);
    pagedView?.dispose();
    pagedView = null;
  };

  /** How a paged file is painted in `mode`: its source a page at a time, a
   *  dotenv file masked as a whole, or what the viewer shows whole once every
   *  byte is held. Null for what shows as its size alone. */
  const pagedPainter = (path, file, mode) => {
    if (shouldMaskDotenv(path, mode, sourceOverride)) {
      return wholeTextPainter((content, text) => {
        const dotenv = renderDotenvSourceHtml(text);
        content.innerHTML = dotenv.html;
        wireDotenvSpoilers(dotenv.secrets);
      });
    }
    if (sourceOverride || mode === "source" || mode === "markdown") return sourceLinesPainter(langForPath(path));
    if (mode === "binary" || mode === "toolarge") return null;
    const media = fileBodyReading(file.mime) === "media" || mode === "svg";
    return wholeBytesPainter((content, body) => {
      releasePreviewMedia(content);
      content.innerHTML = previewBodyHtml(path, { ...file, content_b64: media ? "" : body }, false);
      if (media) bindPreviewMedia(content, file, createMediaBody(body, file.mime));
    }, { asPages: media });
  };

  /** A file kept as pages is painted from them, never from an answer. */
  const paintPagedFile = (host, path, file) => {
    const mode = previewModeFor(file.mime, file.truncated);
    const painter = pagedPainter(pathOf(path), file, mode);
    if (!painter) {
      host.innerHTML = sizePlaceholder(mode, file.size);
      return;
    }
    pagedView = mountPagedFile(host, {
      head: fileAddress(path),
      file,
      readPage: pageReader(path, file),
      restart: () => void restartPagedFile(path, file).catch(() => {}),
      painter,
      releaseCompletedPages: !sourceOverride && (fileBodyReading(file.mime) === "media" || mode === "svg"),
      // A link to a line lands once the page holding it is painted, and a
      // new version's reader is put back where they were.
      onPaint: () => {
        scrollRequestedLineIntoView(path);
        restoreCarriedScroll(host);
      },
    });
  };

  const paintReadingMode = (path, snapshot) => {
    const host = previewEl.querySelector(".file-reading-layer");
    const file = previewFile(snapshot);
    const mode = previewModeFor(file.mime, file.truncated);
    sourceOverride = snapshot.mode === "source";
    stopPagedView();
    if (file.paged) return paintPagedFile(host, path, file);
    const dotenv = shouldMaskDotenv(pathOf(path), mode, sourceOverride)
      ? renderDotenvSourceHtml(snapshot.value)
      : null;
    const truncNotice = dotenv && file.truncated ? `<div class="ftrunc">truncated at 1 MiB</div>` : "";
    host.innerHTML = dotenv ? dotenv.html + truncNotice : previewBodyHtml(pathOf(path), file, sourceOverride);
    bindWholeMedia(host, file, mode, sourceOverride, dotenv);
    if (dotenv) wireDotenvSpoilers(dotenv.secrets);
    scrollRequestedLineIntoView(path);
  };

  const showViewerMode = (path) => {
    const snapshot = viewerState.snapshot();
    const editing = snapshot.mode === "edit";
    previewEl.querySelector(".file-reading-layer").hidden = editing;
    previewEl.querySelector(".file-editor-layer").hidden = !editing;
    const editActions = previewEl.querySelector(".file-edit-actions");
    if (editActions) editActions.hidden = !editing;
    previewEl.querySelectorAll("[data-file-mode]").forEach((button) => {
      const active = button.dataset.fileMode === snapshot.mode;
      button.classList.toggle("active", active);
      button.setAttribute("aria-selected", String(active));
    });
    if (editing && !editor) paintEditor(path, snapshot);
    else if (!editing) paintReadingMode(path, snapshot);
    paintEditStatus();
    if (editing) publishEditorSelection();
  };

  const paintViewer = (path) => {
    stopPagedView();
    const snapshot = viewerState.snapshot();
    previewEl.classList.remove("idle");
    const actions = snapshot.modes.includes("edit") ? '<div class="file-edit-actions" hidden><span class="file-dirty" hidden>Unsaved</span><span class="file-save-status"></span><button type="button" class="btn mini file-reload" hidden>Reload</button><button type="button" class="btn mini file-save">Save</button></div>' : "";
    const tray = fileModeTrayHtml(snapshot.modes, snapshot.mode);
    const file = snapshot.file;
    previewEl.innerHTML = `
      <div class="fphead"><span class="fppath mono">${esc(pathOf(path))}</span><span class="fpsize mono">${Number(file.size) || 0} bytes</span>${tray}${actions}</div>
      <div class="fpbody file-reading-layer"></div><div class="fpbody file-editor-layer"></div>`;
    measurePreviewHead();
    previewEl.querySelectorAll("[data-file-mode]").forEach((button) => {
      button.onclick = () => {
        if (editor) viewerState.edit(viewerState.snapshot().value, editor.selection());
        viewerState.choose(button.dataset.fileMode);
        showViewerMode(path);
      };
    });
    showViewerMode(path);
  };

  const renderPreview = (path, file) => {
    viewingContext?.clearSelection?.();
    const text = fileViewerModes(file).length ? decodeBase64Text(file.content_b64) : "";
    viewerState = createFileViewerState({ file, text });
    paintViewer(path);
  };

  const scrollRequestedLineIntoView = (path) => {
    if (!requestedLine || requestedLine.path !== path) return;
    const row = previewEl.querySelector(`.fsrc tr[data-new-line="${requestedLine.line}"]`);
    if (!row || typeof row.scrollIntoView !== "function") return;
    requestedLine = null;
    row.scrollIntoView({ block: "center" });
  };

  /** Keep the active file's viewer while another tab is shown, when it holds
   *  edits or has a save out — the editor's caret with it. */
  const holdActiveDraft = () => {
    if (!selectedPath || !viewerState?.snapshot().unsaved) return;
    if (editor) viewerState.edit(viewerState.snapshot().value, editor.selection());
    drafts.set(selectedPath, viewerState);
  };

  /** Show a held draft again, and read its file's record: a change on disk
   *  from before the switch, or while it was away, is warned of again. */
  const resumeDraft = (path, state) => {
    drafts.delete(path);
    const request = ++fileRequest;
    beginFileSelection(path);
    viewerState = state;
    paintViewer(path);
    watchFile(path, request);
    void rereadSelectedFile(path, request);
  };

  const showNothing = () => {
    fileRequest += 1;
    stopPagedView();
    unwatchFile?.();
    unwatchFile = null;
    editor?.dispose();
    editor = null;
    viewerState = null;
    selectedPath = null;
    onFileOpen?.(null);
    tree.setOpenPath(null);
    drawer.refresh();
    viewingContext?.clear?.();
    showIdle();
  };

  /** The active tab moved: show its file — its held edits if it has any, else
   *  its record. */
  const showFile = (path) => {
    holdActiveDraft();
    if (!path) return showNothing();
    const draft = drafts.get(path);
    if (draft) return resumeDraft(path, draft);
    void selectFile(path);
  };

  /** A tab closed for good (its edits already confirmed away): nothing it
   *  held may come back as a draft. */
  const forgetFile = (path) => {
    drafts.delete(path);
    if (path === selectedPath) viewerState?.revert();
  };

  // The checkout's own UI state (open tabs, expanded directories), or — across
  // a workspace's roots — the workspace's.
  const layoutAddress = (sub) =>
    roots ? cacheScope?.address({ entityId: layoutEntityId, kind: "ui-files", sub }) || null : rootAddress(checkout.roots[0], "ui-files", sub);

  const finePointer = () => window.matchMedia?.("(pointer: fine)").matches === true;

  /** One root's explorer: its listings, its expanded set, its reads. */
  const treeFor = (root) => ({
    listingAddress: (dir) => rootAddress(root, "tree", dir),
    stateAddress: rootAddress(root, "ui-files", "tree"),
    readsForItself: () => rootReadsForItself(root),
    keepHeldOnError: Boolean(root.scope?.project_id),
    listDirectory: (dir) => callRpc("fs.tree", { ...root.scope, path: dir }),
    finePointer,
  });
  const openFromTree = (key) => {
    drawer.close();
    void tabs.open(key);
  };

  const tree = roots
    ? mountFileRoots(treeListEl, { roots, collapsedAddress: layoutAddress("roots"), treeFor, onOpen: openFromTree })
    : mountFileTree(treeListEl, { ...treeFor(checkout.roots[0]), onOpen: (path) => openFromTree(path) });

  const tabs = mountFileTabs(tabStripEl, {
    stateAddress: layoutAddress("tabs"),
    dirtyPaths,
    confirmClose: (path) => confirmDiscard([path]),
    onClose: forgetFile,
    onShow: showFile,
    initial: openKey,
    locate,
  });

  if (openKey) void tree.reveal(openKey);

  return {
    dispose() {
      disposed = true;
      fileRequest += 1;
      tree.dispose();
      tabs.dispose();
      unwatchFile?.();
      unwatchFile = null;
      stopPagedView();
      stopPreviewHeadMeasurement();
      editor?.dispose();
      viewingContext?.clear?.();
      document.removeEventListener("selectionchange", onDocumentSelectionChange);
      window.removeEventListener("beforeunload", onBeforeUnload);
      drawer.dispose();
    },
    canLeave: discardDirty,
    hasUnsavedChanges: () => dirtyPaths().size > 0,
    retargetScope: (nextScope) => {
      scope = nextScope;
      tree.relist();
      if (selectedPath) {
        const request = ++fileRequest;
        watchFile(selectedPath, request);
        void rereadSelectedFile(selectedPath, request);
      }
    },
  };
}

/** The key the file a route names is held by, or null for none: its path in a
 *  single checkout, its root and path across a workspace's (the route's root,
 *  else the first). */
function openedKey(checkout, openAt) {
  if (!openAt?.path) return null;
  const root = checkout.roots.find((candidate) => candidate.id === openAt.rootId) || checkout.roots[0];
  return root ? checkout.keyOf(root, openAt.path) : null;
}

export { FS_READ_MAX_BYTES };
