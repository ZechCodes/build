// The Files tab — a worktree explorer shared by all three surfaces (task,
// external worktree, plain folder). Left pane: the checkout's tree, with
// directories expanding in place (core/fileTree.js). Right pane: a strip of
// open-file tabs (core/fileTabs.js) over a per-type preview of the active one.
// Which directories are expanded, which files are open and which one is active
// are UI state, remembered per checkout.
//
// Both columns read the cache. A directory's listing is its `tree` record, and
// a `files` push rewriting one moves the tree under the reader; a file's body
// is its `file` record, and one the cache holds opens with no round trip. Two
// reads are left on the wire and both write through: `fs.tree` for a directory
// nothing has ever been written for, and `fs.read` for a file nothing holds.
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
// images render via `<img src="data:...">` — never inlined into the DOM. The
// server fences the scope root and every path; this view never sends host paths.

import { esc, pickAFileText } from "../core/text.js";
import { directoryCacheId, syncWalksCheckout } from "../core/directoryScope.js";
import { deleteCached, readCached, subscribeCache } from "../core/localCache.js";
import { FILE_RECORD_KIND, cacheFileBody } from "../core/cacheLifetime.js";
import { renderMarkdown } from "../core/markdown.js";
import { highlightCode, langForPath } from "../core/highlight.js";
import { initPaneDrawer, paneDrawerHtml } from "../core/paneDrawer.js";
import { isDotenvPath, renderDotenvSourceHtml, SPOILER_DOTS } from "../core/secrets.js";
import { confirmAction } from "../core/confirm.js";
import { createFileViewerState, encodeBase64Text, fileModeTrayHtml, fileViewerModes, sameFile } from "../core/fileViewer.js";
import { mountFileEditor } from "../core/fileEditor.js";
import { captureFileSelection } from "../core/fileSelection.js";
import { mountMeasuredHeight } from "../core/measuredInset.js";
import { mountFileTree } from "../core/fileTree.js";
import { mountFileTabs } from "../core/fileTabs.js";

const FS_READ_MAX_BYTES = 1_048_576;

/** Pure: the preview mode for a server `mime` hint + `truncated` flag. A
 *  truncated image/html/svg is garbage as a partial, so it demotes to a
 *  size placeholder ("toolarge"); markdown/source render what arrived. */
// eslint-disable-next-line complexity -- ratchet: previewModeFor is at 18, cap 10 — reduce it, then drop this line
export function previewModeFor(mime, truncated) {
  const base =
    mime === "text/markdown"
      ? "markdown"
      : mime === "text/html"
        ? "html"
        : mime === "image/svg+xml"
          ? "svg"
          : (mime || "").startsWith("image/")
            ? "image"
            : (mime || "").startsWith("audio/")
              ? "audio"
              : (mime || "").startsWith("video/")
                ? "video"
            : mime === "application/octet-stream" || mime === "application/pdf"
              ? "binary"
              : "source";
  if (truncated && (base === "html" || base === "svg" || base === "image" || base === "audio" || base === "video")) return "toolarge";
  return base;
}

/** Whether a preview mode offers a "view source" toggle (rendered ⇄ raw). */
export function previewHasSourceToggle(mode) {
  return mode === "markdown" || mode === "html" || mode === "svg";
}

/** Pure: decode a base64 payload to a UTF-8 string (for text/source previews). */
export function decodeBase64Text(contentB64) {
  const bytes = Uint8Array.from(atob(contentB64 || ""), (c) => c.charCodeAt(0));
  return new TextDecoder().decode(bytes);
}

/** Pure: the syntax-highlighted source view for a file. Code is highlighted by
 *  the path's extension (langForPath) and, for an unknown extension, falls back
 *  to escaped plain text — highlightCode never emits a live tag either way. */
export function sourcePreviewHtml(path, text) {
  const lang = langForPath(path);
  const rows = text
    .split("\n")
    .map((line, index) => `<tr data-new-line="${index + 1}"><td class="fsrc-ln">${index + 1}</td><td class="fsrc-code"><code>${highlightCode(line, lang) || " "}</code></td></tr>`)
    .join("");
  return `<div class="fsrc"><table>${rows}</table></div>`;
}

export function mediaPreviewHtml(mode, mime, contentB64) {
  const tag = mode === "audio" ? "audio" : "video";
  const className = mode === "audio" ? "faudio" : "fvideo";
  return `<${tag} class="fmedia ${className}" controls preload="metadata" src="data:${esc(mime)};base64,${contentB64}"></${tag}>`;
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
  if (mode === "markdown") return `<div class="plan">${renderMarkdown(decodeBase64Text(file.content_b64))}</div>${truncNotice}`;
  if (mode === "html") return `<iframe class="fhtml" sandbox="" src="data:text/html;base64,${file.content_b64}"></iframe>`;
  if (mode === "svg") return `<img class="fimg" src="data:image/svg+xml;base64,${file.content_b64}" alt="">`;
  if (mode === "image") return `<img class="fimg" src="data:${esc(file.mime)};base64,${file.content_b64}" alt="" style="max-width:100%">`;
  if (mode === "audio" || mode === "video") return mediaPreviewHtml(mode, file.mime, file.content_b64);
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

/**
 * renderFilesTab(body, { scope, callRpc, cacheScope }) — mount the browser into
 * `body`. `scope` is the plain server-resolved scope object ({task_id} /
 * {project_id[, worktree_id]}) spread into every fs.* call; `callRpc(method,
 * params)` is the app RPC (fs.* ride the app session, not the terminal socket);
 * `cacheScope` is the cache of the machine that checkout is on, handed down by
 * the view, and a mount without one saves nothing. No polling — fetches only on
 * navigation/selection. Returns { dispose() }.
 */
export function renderFilesTab(body, { scope, callRpc, cacheScope = null, openAt = null, onFileOpen = null, viewingContext = null }) {
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
    previewEl.classList.add("idle");
    previewEl.innerHTML = previewPlaceholderHtml(kind, message, hint);
  };
  // Below the stacking width the tree is behind the drawer's trigger row rather
  // than beside the preview, so the empty state names it instead of pointing at
  // it.
  const showIdle = () => showPlaceholder("idle", "No file open", "Choose a file from the tree to read it here.");
  showIdle();

  let requestedLine = openAt && openAt.line ? { path: openAt.path, line: openAt.line } : null;
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
    summary: () => selectedPath || pickAFileText,
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
    intro: `Your unsaved changes to ${[...paths].join(", ")} will be lost.`,
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
    viewingContext.set({ version: 1, items: [{ kind: "file", path: selectedPath }] });
  };

  const publishContextSelection = (items) => {
    if (!viewingContext) return;
    if (viewingContext.setSelection) viewingContext.setSelection(items);
    else viewingContext.set({ version: 1, items: [{ kind: "file", path: selectedPath }, ...items] });
  };

  const publishEditorSelection = () => {
    const snapshot = viewerState?.snapshot();
    const items = [];
    if (snapshot?.mode === "edit" && snapshot.selection.end > snapshot.selection.start) {
      items.push({
        kind: "selection",
        path: selectedPath,
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
    const items = captureFileSelection(readingLayer, selectedPath, selection);
    if (items.length) {
      publishContextSelection(items);
      return;
    }
    if (!composerHasFocus() && selectionBelongsToReadingLayer(selection, readingLayer)) publishContextSelection([]);
  };
  document.addEventListener("selectionchange", onDocumentSelectionChange);

  // The local cache's address for one directory's listing. A project-scoped
  // one names no entity and takes no part.
  const cacheEntityId = () => directoryCacheId(scope);

  /** Whether this tab is the only reader of its checkout. A run's or an
   *  external worktree's records are kept true by the sync layer, so what they
   *  hold is the answer. A workspace source's and a project's are not walked by
   *  anybody, so what they hold is the last visit's own work: a seed to paint
   *  at once, and never a reason to skip the read. */
  const readsForItself = () => !syncWalksCheckout(scope);
  const treeAddress = (path) =>
    cacheEntityId() ? cacheScope?.address({ entityId: cacheEntityId(), kind: "tree", sub: path }) || null : null;

  const fileAddress = (path) =>
    cacheEntityId() ? cacheScope?.address({ entityId: cacheEntityId(), kind: FILE_RECORD_KIND, sub: path }) || null : null;

  const heldRecord = (address) => (address ? readCached(address) : Promise.resolve(undefined));

  let unwatchFile = null;

  const stillSelected = (request, path) =>
    !disposed && request === fileRequest && selectedPath === path;

  const adoptFile = (path, file) => {
    editor?.dispose();
    editor = null;
    renderPreview(path, file);
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

  const storedFileResult = async (address, file) => {
    const stored = await heldRecord(address);
    return stored?.value?.file ? { file: stored.value.file } : { file, direct: true };
  };

  const discardSupersededFile = async (address, previousAt) => {
    const after = await heldRecord(address);
    if (after && after.at === previousAt) await deleteCached([address]);
  };

  /** Store one pulled body and then read the stored record back. The narrow
   *  direct result is the existing oversized/truncated exception: retention
   *  policy forbids that body from entering IndexedDB, pending its owner
   *  decision. */
  const storePulledFile = async (path, file, request, previousAt) => {
    const address = fileAddress(path);
    if (!address) return { file, direct: true };
    const current = await heldRecord(address);
    if (!stillSelected(request, path)) return {};
    if (current?.at !== previousAt) return { file: current?.value?.file };
    const kept = await cacheFileBody({ deviceId: address.deviceId, entityId: address.entityId, path, file });
    if (!stillSelected(request, path)) return {};
    if (kept) return storedFileResult(address, file);
    await discardSupersededFile(address, previousAt);
    return { file, direct: true };
  };

  const pullFile = async (path, request, previousAt) => {
    try {
      const file = await callRpc("fs.read", { ...scope, path });
      return storePulledFile(path, file, request, previousAt);
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
    onFileOpen?.(path);
    tree.setOpenPath(path);
    sourceOverride = false;
    editor?.dispose();
    editor = null;
    viewerState = null;
    selectedPath = path;
    drawer.refresh();
    viewingContext?.clearSelection?.();
    publishFileContext();
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
    if (readsForItself()) return false;
    if (address) void cacheFileBody({ deviceId: address.deviceId, entityId: address.entityId, path, file: held });
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
    const result = await pullFile(path, request, record?.at);
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
        ...scope,
        path,
        content_b64: encodeBase64Text(write.value),
        expected_revision: write.revision,
      });
    } catch (error) {
      state.saveFailed(error);
      return afterSave(path, state, null);
    }
    state.saveSucceeded(written);
    afterSave(path, state, written);
    await storeWrittenFile(path, address, written, baseline, (await before)?.at);
  };

  /** A cache access refresh changes the timestamp without changing the file.
   *  Replace the submitted baseline, but keep a competing revision visible.
   *  If the record disappeared while saving, preserve that invalidation too. */
  const storeWrittenFile = async (path, address, written, baseline, previousAt) => {
    if (!address) return false;
    const current = await heldRecord(address);
    if (current?.value?.file) {
      if (!sameFile(current.value.file, baseline)) return false;
    } else if (current?.at !== previousAt) return false;
    const kept = await cacheFileBody({ deviceId: address.deviceId, entityId: address.entityId, path, file: written });
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
    const result = await pullFile(path, request, before?.at);
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

  const previewFile = (snapshot) => snapshot.modes.length ? ({
      ...snapshot.file,
      content_b64: encodeBase64Text(snapshot.value),
      size: new TextEncoder().encode(snapshot.value).length,
    }) : snapshot.file;

  const paintReadingMode = (path, snapshot) => {
    const host = previewEl.querySelector(".file-reading-layer");
    const file = previewFile(snapshot);
    const mode = previewModeFor(file.mime, file.truncated);
    sourceOverride = snapshot.mode === "source";
    const dotenv = shouldMaskDotenv(path, mode, sourceOverride)
      ? renderDotenvSourceHtml(snapshot.value)
      : null;
    const truncNotice = dotenv && file.truncated ? `<div class="ftrunc">truncated at 1 MiB</div>` : "";
    host.innerHTML = dotenv ? dotenv.html + truncNotice : previewBodyHtml(path, file, sourceOverride);
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
    const snapshot = viewerState.snapshot();
    previewEl.classList.remove("idle");
    const actions = snapshot.modes.includes("edit") ? '<div class="file-edit-actions" hidden><span class="file-dirty" hidden>Unsaved</span><span class="file-save-status"></span><button type="button" class="btn mini file-reload" hidden>Reload</button><button type="button" class="btn mini file-save">Save</button></div>' : "";
    const tray = fileModeTrayHtml(snapshot.modes, snapshot.mode);
    const file = snapshot.file;
    previewEl.innerHTML = `
      <div class="fphead"><span class="fppath mono">${esc(path)}</span><span class="fpsize mono">${Number(file.size) || 0} bytes</span>${tray}${actions}</div>
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

  const uiStateAddress = (sub) =>
    cacheEntityId() ? cacheScope?.address({ entityId: cacheEntityId(), kind: "ui-files", sub }) || null : null;

  const finePointer = () => window.matchMedia?.("(pointer: fine)").matches === true;

  const tree = mountFileTree(treeListEl, {
    listingAddress: treeAddress,
    stateAddress: uiStateAddress("tree"),
    readsForItself,
    listDirectory: (path) => callRpc("fs.tree", { ...scope, path }),
    finePointer,
    onOpen: (path) => {
      drawer.close();
      void tabs.open(path);
    },
  });

  const tabs = mountFileTabs(tabStripEl, {
    stateAddress: uiStateAddress("tabs"),
    dirtyPaths,
    confirmClose: (path) => confirmDiscard([path]),
    onClose: forgetFile,
    onShow: showFile,
    initial: openAt?.path || null,
  });

  if (openAt) void tree.reveal(openAt.path);

  return {
    dispose() {
      disposed = true;
      fileRequest += 1;
      tree.dispose();
      tabs.dispose();
      unwatchFile?.();
      unwatchFile = null;
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

export { FS_READ_MAX_BYTES };
