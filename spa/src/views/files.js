// The Files tab — a worktree browser shared by all three surfaces (task,
// external worktree, plain folder). Left pane: one-directory-at-a-time tree
// with a breadcrumb and a `..` row below the root. Right pane: a per-type
// preview of the selected file.
//
// Both columns read the cache. A directory's listing is its `tree` record, and
// a `files` push rewriting one moves the tree under the reader; a file's body
// is its `file` record, and one the cache holds opens with no round trip. Two
// reads are left on the wire and both write through: `fs.tree` for a directory
// nothing has ever been written for, and `fs.read` for a file nothing holds.
//
// SECURITY: every name/path is escaped. HTML previews render in a
// `sandbox=""` iframe over a `data:` URL (no scripts, no same-origin); SVG and
// images render via `<img src="data:...">` — never inlined into the DOM. The
// server fences the scope root and every path; this view never sends host paths.

import { esc, pickAFileText } from "../core/text.js";
import { directoryCacheId } from "../core/directoryScope.js";
import { deleteCached, readCached, subscribeCache, writeCached } from "../core/localCache.js";
import { FILE_RECORD_KIND, cacheFileBody } from "../core/cacheLifetime.js";
import { renderMarkdown } from "../core/markdown.js";
import { highlightCode, langForPath } from "../core/highlight.js";
import { initPaneDrawer, paneDrawerHtml } from "../core/paneDrawer.js";
import { isDotenvPath, renderDotenvSourceHtml, SPOILER_DOTS } from "../core/secrets.js";
import { confirmAction } from "../core/confirm.js";
import { createFileViewerState, encodeBase64Text, fileModeTrayHtml, fileViewerModes } from "../core/fileViewer.js";
import { mountFileEditor } from "../core/fileEditor.js";
import { captureFileSelection } from "../core/fileSelection.js";
import { mountMeasuredHeight } from "../core/measuredInset.js";

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

const joinPath = (dir, name) => (dir ? `${dir}/${name}` : name);
const parentPath = (dir) => dir.split("/").slice(0, -1).join("/");

/** Pure: the tree pane's HTML for one directory listing — breadcrumb, an `..`
 *  row below the root, then dirs/files/symlinks. Repo file names are untrusted
 *  input (spec §9): every name is escaped, in row labels AND in the data-dir/
 *  data-file attributes the click wiring reads back. */
export function filesTreeHtml(dir, entries) {
  const crumb = `<div class="fcrumb mono">${dir ? esc(dir) : "/"}</div>`;
  const up = dir ? `<div class="frow fup" data-up="1"><span class="fk">↰</span> ..</div>` : "";
  // The name is its own element in every row: the tree is a fixed-width column
  // that gives ground rather than growing, so a long unbroken name has to
  // ellipsize inside it (.fname), and a bare text node in the row's flex line
  // has no box to do that in.
  const rows = entries
    .map((entry) => {
      const name = `<span class="fname">${esc(entry.name)}</span>`;
      if (entry.kind === "dir")
        return `<div class="frow fdir" data-dir="${esc(entry.name)}"><span class="fk">▸</span> ${name}</div>`;
      if (entry.kind === "symlink")
        return `<div class="frow fsym" title="symlink — not followed"><span class="fk">↳</span> ${name}</div>`;
      return `<div class="frow ffile" data-file="${esc(entry.name)}"><span class="fk">·</span> ${name}<span class="fsize mono">${Number(entry.size) || 0}</span></div>`;
    })
    .join("");
  return crumb + (up + rows || '<div class="empty">Empty directory.</div>');
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
  // pins to the bottom of); `.ftree-list` is the part `loadTree` replaces —
  // splitting them is what lets a directory change repaint the rows without
  // taking the tab bar below them with it.
  body.innerHTML = `<div class="files pane-split"><div class="ftree pane-list" id="ftree"><div class="ftree-list"></div></div><div class="fpreview idle" id="fpreview"></div>${paneDrawerHtml("files")}</div>`;
  const treeEl = body.querySelector("#ftree");
  const treeListEl = body.querySelector(".ftree-list");
  const previewEl = body.querySelector("#fpreview");
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
  showPlaceholder("idle", "No file open", "Choose a file from the tree to read it here.");

  let dir = openAt ? parentPath(openAt.path) : ""; // current directory, relative to the scope root
  let requestedLine = openAt && openAt.line ? { path: openAt.path, line: openAt.line } : null;
  let sourceOverride = false; // per-selected-file "view source" toggle
  let viewerState = null;
  let editor = null;
  let selectedPath = null;
  let savingState = null;
  let fileRequest = 0;

  // On a narrow viewport the tree is a drawer over the preview. Only a file
  // closes it: a directory row is still part of choosing one, and closing the
  // drawer under a tap that changed nothing but the tree would put the choosing
  // away mid-choice. Shut, the trigger over it names the file being read —
  // which the preview's own header says, and the preview is behind the drawer.
  const drawer = initPaneDrawer(body.querySelector(".files"), {
    list: treeEl,
    closeOnSelect: ".ffile",
    summary: () => selectedPath || pickAFileText,
  });

  const onBeforeUnload = (event) => {
    if (!viewerState?.snapshot().dirty) return;
    event.preventDefault();
    event.returnValue = "";
  };
  window.addEventListener("beforeunload", onBeforeUnload);

  const discardDirty = async () => !viewerState?.snapshot().dirty || confirmAction({
    title: "Discard file edits?",
    intro: `Your unsaved changes to ${selectedPath} will be lost.`,
    confirmLabel: "Discard edits",
    danger: true,
  });

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
        ...(snapshot.dirty ? { unsaved: true } : {}),
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

  const renderTree = (entries) => {
    treeListEl.innerHTML = filesTreeHtml(dir, entries);
    if (dir) treeEl.querySelector(".fup").onclick = () => loadTree(parentPath(dir));
    treeEl.querySelectorAll(".fdir").forEach((row) => (row.onclick = () => loadTree(joinPath(dir, row.dataset.dir))));
    treeEl.querySelectorAll(".ffile").forEach((row) => (row.onclick = () => selectFile(joinPath(dir, row.dataset.file), row)));
  };

  // The local cache's address for one directory's listing. A project-scoped
  // one names no entity and takes no part.
  const cacheEntityId = () => directoryCacheId(scope);
  const treeAddress = (path) =>
    cacheEntityId() ? cacheScope?.address({ entityId: cacheEntityId(), kind: "tree", sub: path }) || null : null;

  const fileAddress = (path) =>
    cacheEntityId() ? cacheScope?.address({ entityId: cacheEntityId(), kind: FILE_RECORD_KIND, sub: path }) || null : null;

  const heldValue = async (address) => (address ? (await readCached(address))?.value : undefined);

  let treeRequest = 0; // which navigation the paints below still speak for
  let unwatchTree = null; // the watch on the listing on screen

  const paintListing = (listing) => {
    dir = listing.path || "";
    renderTree(listing.entries || []);
  };

  /** Hear this directory's record move: a `files` push rewrites the root
   *  listing, and the sync layer re-lists whichever deeper ones the reader
   *  walked into. Only the listing on screen is watched — the reader walking
   *  away takes the watch with them. */
  const watchListing = (path, request) => {
    unwatchTree?.();
    unwatchTree = null;
    const address = treeAddress(path);
    if (!address) return;
    unwatchTree = subscribeCache(address, () => void rereadListing(path, request));
  };

  /** Whether the paints below still speak for where the reader is standing. */
  const stillListing = (request) => !disposed && request === treeRequest;

  const rereadListing = async (path, request) => {
    if (!stillListing(request)) return;
    const held = await heldValue(treeAddress(path));
    if (stillListing(request) && held) paintListing(held);
  };

  const cannotListHtml = (error) => `<div class="empty">cannot list: ${esc((error && error.message) || "error")}</div>`;

  /** The one on-demand listing: a directory nothing has ever been written
   *  for. It is written through, so the sync layer keeps it fresh from here. */
  const listTree = async (nextDir, request) => {
    let res;
    try {
      res = await callRpc("fs.tree", { ...scope, path: nextDir });
    } catch (e) {
      if (stillListing(request)) treeListEl.innerHTML = cannotListHtml(e);
      return;
    }
    if (!stillListing(request)) return;
    const listing = { path: res.path || "", entries: res.entries || [] };
    paintListing(listing);
    const address = treeAddress(nextDir);
    if (address) writeCached(address, listing);
  };

  const loadTree = async (nextDir) => {
    const request = ++treeRequest;
    watchListing(nextDir, request);
    const held = await heldValue(treeAddress(nextDir));
    if (!stillListing(request)) return;
    if (held) {
      paintListing(held);
      return;
    }
    await listTree(nextDir, request);
  };

  /** Keep what the reader just opened, under the recent-files rule. A body too
   *  big for the cache is shown and not kept (core/cacheLifetime.js). */
  const keepFileBody = (path, file) => {
    const address = fileAddress(path);
    if (!address) return;
    // Fire and forget, and forgiving: a disk that will not take the body is a
    // cold second look, never something the reader is told about.
    void cacheFileBody({ deviceId: address.deviceId, entityId: address.entityId, path, file }).catch(() => {});
  };

  /** The saved body is stale the moment this tab writes over it, and the write
   *  answers with a revision rather than with the file. Let it go: the next
   *  open reads the file the save made. */
  const dropFileBody = (path) => {
    const address = fileAddress(path);
    if (address) void deleteCached([address]);
  };

  /** One file's body: the record where there is one, else the one read off the
   *  wire, written through. `fresh` is the reload verb, which exists to go
   *  past whatever is held. */
  const readFile = async (path, { fresh = false } = {}) => {
    const held = fresh ? undefined : (await heldValue(fileAddress(path)))?.file;
    if (held) {
      // Opening it is what makes it recent — the five kept are the five last
      // read, not the five first read, or the file the reader keeps coming
      // back to is the one the trim drops.
      keepFileBody(path, held);
      return { file: held };
    }
    try {
      const file = await callRpc("fs.read", { ...scope, path });
      keepFileBody(path, file);
      return { file };
    } catch (error) {
      return { error };
    }
  };

  const setText = (element, value) => {
    if (element) element.textContent = value;
  };

  const beginSave = (submittedState) => {
    savingState = submittedState;
    previewEl.querySelector(".file-save").disabled = true;
    setText(previewEl.querySelector(".file-save-status"), "");
    const reload = previewEl.querySelector(".file-reload");
    if (reload) reload.hidden = true;
  };

  const finishSave = (submittedState, written, submittedValue) => {
    submittedState.saved(written, submittedValue);
    savingState = null;
    if (disposed || viewerState !== submittedState) return;
    setText(previewEl.querySelector(".fpsize"), `${Number(written.size) || 0} bytes`);
    setText(treeEl.querySelector(".frow.sel .fsize"), String(Number(written.size) || 0));
    paintEditStatus();
    publishEditorSelection();
  };

  const failSave = (submittedState, error) => {
    savingState = null;
    if (disposed || viewerState !== submittedState) return;
    setText(previewEl.querySelector(".file-save-status"), error.message || "Save failed");
    const reload = previewEl.querySelector(".file-reload");
    if (reload) reload.hidden = !String(error.message).includes("revision conflict");
    previewEl.querySelector(".file-save").disabled = false;
  };

  const beginFileSelection = (path, row) => {
    if (requestedLine?.path !== path) requestedLine = null;
    onFileOpen?.(path);
    treeEl.querySelectorAll(".frow.sel").forEach((selected) => selected.classList.remove("sel"));
    row?.classList.add("sel");
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

  const selectedFileIsCurrent = (request, path) =>
    !disposed && request === fileRequest && selectedPath === path;

  const editorIsCurrent = (path, state) =>
    !disposed && selectedPath === path && viewerState === state;

  const selectFile = async (path, row) => {
    if (disposed || path === selectedPath) return;
    if (!await discardDirty()) return;
    if (disposed || path === selectedPath) return;
    const request = ++fileRequest;
    // The tab names the file it is standing in, so the URL can say so too.
    beginFileSelection(path, row);
    const result = await readFile(path);
    if (!selectedFileIsCurrent(request, path)) return;
    if (result.error) {
      showPlaceholder("error", `cannot read: ${result.error.message || "error"}`);
      return;
    }
    renderPreview(path, result.file);
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

  const saveEditor = async (path) => {
    const submittedState = viewerState;
    if (savingState === submittedState) return;
    const snapshot = submittedState.snapshot();
    const submittedValue = snapshot.value;
    beginSave(submittedState);
    try {
      const written = await callRpc("fs.write", {
        ...scope,
        path,
        content_b64: encodeBase64Text(snapshot.value),
        expected_revision: snapshot.revision,
      });
      dropFileBody(path);
      finishSave(submittedState, written, submittedValue);
    } catch (error) {
      failSave(submittedState, error);
    }
  };

  const reloadEditor = async (path) => {
    const expectedState = viewerState;
    if (!await discardDirty()) return;
    if (!editorIsCurrent(path, expectedState)) return;
    const reloadingState = viewerState;
    const reloadingValue = reloadingState.snapshot().value;
    const request = ++fileRequest;
    const result = await readFile(path, { fresh: true });
    if (!selectedFileIsCurrent(request, path) || viewerState !== reloadingState) return;
    if (reloadingState.snapshot().value !== reloadingValue) return;
    if (result.error) {
      previewEl.querySelector(".file-save-status").textContent = result.error.message || "Reload failed";
      return;
    }
    editor?.dispose();
    editor = null;
    renderPreview(path, result.file);
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

  const paintEditStatus = () => {
    const snapshot = viewerState.snapshot();
    const dirty = previewEl.querySelector(".file-dirty");
    const save = previewEl.querySelector(".file-save");
    if (dirty) dirty.hidden = !snapshot.dirty;
    if (save) save.disabled = !snapshot.dirty || savingState === viewerState;
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

  loadTree(dir).then(() => {
    if (disposed || !openAt) return;
    const fileName = openAt.path.split("/").at(-1);
    const row = [...treeEl.querySelectorAll(".ffile")].find((entry) => entry.dataset.file === fileName);
    selectFile(openAt.path, row || null);
  });

  return {
    dispose() {
      disposed = true;
      treeRequest += 1;
      fileRequest += 1;
      unwatchTree?.();
      unwatchTree = null;
      stopPreviewHeadMeasurement();
      editor?.dispose();
      viewingContext?.clear?.();
      document.removeEventListener("selectionchange", onDocumentSelectionChange);
      window.removeEventListener("beforeunload", onBeforeUnload);
      drawer.dispose();
    },
    canLeave: discardDirty,
    hasUnsavedChanges: () => Boolean(viewerState?.snapshot().dirty),
    retargetScope: (nextScope) => {
      scope = nextScope;
    },
  };
}

export { FS_READ_MAX_BYTES };
