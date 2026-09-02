// The Files tab — a worktree browser shared by all three surfaces (task,
// external worktree, primary "main" checkout). Left pane: one-directory-at-a-time
// tree (fs.tree) with a breadcrumb and a `..` row below the root. Right pane: a
// per-type preview of the selected file (fs.read).
//
// SECURITY: every name/path is escaped. HTML previews render in a
// `sandbox=""` iframe over a `data:` URL (no scripts, no same-origin); SVG and
// images render via `<img src="data:...">` — never inlined into the DOM. The
// server fences the scope root and every path; this view never sends host paths.

import { esc } from "../core/text.js";
import { cacheDeviceId } from "../core/cacheScope.js";
import { readCached, writeCached } from "../core/localCache.js";
import { renderMarkdown } from "../core/markdown.js";
import { highlightCode, langForPath } from "../core/highlight.js";
import { initPaneDrawer, paneDrawerHtml } from "../core/paneDrawer.js";
import { isDotenvPath, renderDotenvSourceHtml, SPOILER_DOTS } from "../core/secrets.js";

const FS_READ_MAX_BYTES = 1_048_576;

/** Pure: the preview mode for a server `mime` hint + `truncated` flag. A
 *  truncated image/html/svg is garbage as a partial, so it demotes to a
 *  size placeholder ("toolarge"); markdown/source render what arrived. */
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
 *  to escaped plain text — highlightCode never emits a live tag either way.
 *  Every row states its line, so a reader arriving from a diff can be taken to
 *  the line the diff was about. */
export function sourcePreviewHtml(path, text) {
  const lang = langForPath(path);
  const rows = text
    .split("\n")
    .map((line, index) => `<tr data-line="${index + 1}"><td class="fsrc-ln">${index + 1}</td><td class="fsrc-code"><code>${highlightCode(line, lang) || " "}</code></td></tr>`)
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
 * renderFilesTab(body, { scope, callRpc }) — mount the browser into `body`.
 * `scope` is the plain server-resolved scope object ({task_id} / {project_id[,
 * worktree_id]}) spread into every fs.* call; `callRpc(method, params)` is the
 * app RPC (fs.* ride the app session, not the terminal socket). No polling —
 * fetches only on navigation/selection. Returns { dispose() }.
 *
 * `initialPath` is where the browser opens: `{ path, line }`, from a link in a
 * conversation or the way out of a diff. The tree opens on the file's own
 * directory, the preview on the file, and — where the line is named and the
 * file reads as source — the view is scrolled to that line.
 */
export function renderFilesTab(body, { scope, callRpc, initialPath = null }) {
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
  // On a narrow viewport the tree is a drawer over the preview. Only a file
  // closes it: a directory row is still part of choosing one, and closing the
  // drawer under a tap that changed nothing but the tree would put the choosing
  // away mid-choice.
  const drawer = initPaneDrawer(body.querySelector(".files"), { list: treeEl, closeOnSelect: ".ffile" });
  // The placeholder states render container-less (no panel box), centered in
  // the preview area; only a loaded file gets the bordered panel back.
  const showPlaceholder = (kind, message, hint) => {
    previewEl.classList.add("idle");
    previewEl.innerHTML = previewPlaceholderHtml(kind, message, hint);
  };
  // Below the stacking width the tree is behind the drawer handle rather than
  // beside the preview, so the empty state names it instead of pointing at it.
  showPlaceholder("idle", "No file open", "Choose a file from the tree to read it here.");

  const openAt = initialPath || null; // { path, line } — where this mount opens
  let dir = openAt ? parentPath(openAt.path) : ""; // current directory, relative to the scope root
  let scrollToLine = openAt && openAt.line ? openAt.line : null; // consumed by the first preview
  let sourceOverride = false; // per-selected-file "view source" toggle

  const renderTree = (entries) => {
    treeListEl.innerHTML = filesTreeHtml(dir, entries);
    if (dir) treeEl.querySelector(".fup").onclick = () => loadTree(parentPath(dir));
    treeEl.querySelectorAll(".fdir").forEach((row) => (row.onclick = () => loadTree(joinPath(dir, row.dataset.dir))));
    treeEl.querySelectorAll(".ffile").forEach((row) => (row.onclick = () => selectFile(joinPath(dir, row.dataset.file), row)));
  };

  // The local cache's address for one directory's listing. A primary checkout
  // names no entity and takes no part.
  const cacheEntityId = (scope && (scope.run_id || scope.worktree_id)) || null;
  const treeAddress = (path) => {
    const deviceId = cacheDeviceId();
    return deviceId && cacheEntityId ? { deviceId, entityId: cacheEntityId, kind: "tree", sub: path } : null;
  };

  let treeRequest = 0; // which navigation the paints below still speak for
  let liveRenderedRequest = 0; // a live answer outranks the cache for its request
  let cachePaintedRequest = 0; // whether the cache already painted this request

  const loadTree = async (nextDir) => {
    const request = ++treeRequest;
    const address = treeAddress(nextDir);
    if (address) {
      // The saved listing paints while the machine is being asked — never over
      // a live answer, never for a directory the reader has already left. A
      // machine that then cannot answer leaves the saved listing standing.
      readCached(address).then((record) => {
        if (!record || request !== treeRequest || liveRenderedRequest === request) return;
        cachePaintedRequest = request;
        dir = record.value.path || "";
        renderTree(record.value.entries || []);
      });
    }
    let res;
    try {
      res = await callRpc("fs.tree", { ...scope, path: nextDir });
    } catch (e) {
      if (request !== treeRequest || cachePaintedRequest === request) return;
      treeListEl.innerHTML = `<div class="empty">cannot list: ${esc((e && e.message) || "error")}</div>`;
      return;
    }
    if (request !== treeRequest) return;
    liveRenderedRequest = request;
    dir = res.path || "";
    renderTree(res.entries || []);
    if (address) writeCached(address, { path: dir, entries: res.entries || [] });
  };

  const selectFile = async (path, row) => {
    treeEl.querySelectorAll(".frow.sel").forEach((r) => r.classList.remove("sel"));
    if (row) row.classList.add("sel");
    sourceOverride = false;
    showPlaceholder("loading");
    let file;
    try {
      file = await callRpc("fs.read", { ...scope, path });
    } catch (e) {
      showPlaceholder("error", `cannot read: ${(e && e.message) || "error"}`);
      return;
    }
    renderPreview(path, file);
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

  const renderPreview = (path, file) => {
    previewEl.classList.remove("idle");
    const mode = previewModeFor(file.mime, file.truncated);
    const canToggle = previewHasSourceToggle(mode);
    const toggle = canToggle
      ? `<button class="btn mini" id="fsrctoggle">${sourceOverride ? "view rendered" : "view source"}</button>`
      : "";
    // A dotenv file's source view masks secret-like values. renderDotenvSourceHtml
    // keeps every masked value OUT of the returned HTML (dots only) — the values
    // ride back in `secrets` and are wired in after mount.
    const dotenv = shouldMaskDotenv(path, mode, sourceOverride)
      ? renderDotenvSourceHtml(decodeBase64Text(file.content_b64))
      : null;
    const truncNotice = dotenv && file.truncated ? `<div class="ftrunc">truncated at 1 MiB</div>` : "";
    const revealAll =
      dotenv && dotenv.secrets.length ? `<button class="btn mini" id="fpreveal">Reveal all</button>` : "";
    previewEl.innerHTML = `
      <div class="fphead"><span class="fppath mono">${esc(path)}</span><span class="fpsize mono">${Number(file.size) || 0} bytes</span>${revealAll}${toggle}</div>
      <div class="fpbody">${dotenv ? dotenv.html + truncNotice : previewBodyHtml(path, file, sourceOverride)}</div>`;
    if (dotenv) wireDotenvSpoilers(dotenv.secrets);
    scrollRequestedLineIntoView();
    const toggleBtn = previewEl.querySelector("#fsrctoggle");
    if (toggleBtn)
      toggleBtn.onclick = () => {
        sourceOverride = !sourceOverride;
        renderPreview(path, file);
      };
  };

  /** The line the reader was sent to, put in the middle of the view — once.
   *  A file with no source rows to land on (a rendered preview, a binary)
   *  keeps the request until one is drawn. */
  const scrollRequestedLineIntoView = () => {
    if (!scrollToLine) return;
    const row = previewEl.querySelector(`.fsrc tr[data-line="${scrollToLine}"]`);
    if (!row || typeof row.scrollIntoView !== "function") return;
    scrollToLine = null;
    row.scrollIntoView({ block: "center" });
  };

  loadTree(dir).then(() => {
    if (!openAt) return;
    const fileName = openAt.path.split("/").at(-1);
    const row = [...treeEl.querySelectorAll(".ffile")].find((entry) => entry.dataset.file === fileName);
    selectFile(openAt.path, row || null);
  });

  return { dispose: () => drawer.dispose() };
}

export { FS_READ_MAX_BYTES };
