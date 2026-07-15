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
import { renderMarkdown } from "../core/markdown.js";
import { highlightCode, langForPath } from "../core/highlight.js";

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
            : mime === "application/octet-stream" || mime === "application/pdf"
              ? "binary"
              : "source";
  if (truncated && (base === "html" || base === "svg" || base === "image")) return "toolarge";
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
  const rows = entries
    .map((entry) => {
      if (entry.kind === "dir")
        return `<div class="frow fdir" data-dir="${esc(entry.name)}"><span class="fk">▸</span> ${esc(entry.name)}</div>`;
      if (entry.kind === "symlink")
        return `<div class="frow fsym" title="symlink — not followed"><span class="fk">↳</span> ${esc(entry.name)}</div>`;
      return `<div class="frow ffile" data-file="${esc(entry.name)}"><span class="fk">·</span> ${esc(entry.name)}<span class="fsize mono">${Number(entry.size) || 0}</span></div>`;
    })
    .join("");
  return crumb + (up + rows || '<div class="empty">Empty directory.</div>');
}

/** Pure: the syntax-highlighted source view for a file. Code is highlighted by
 *  the path's extension (langForPath) and, for an unknown extension, falls back
 *  to escaped plain text — highlightCode never emits a live tag either way. */
export function sourcePreviewHtml(path, text) {
  return `<pre class="fsrc"><code>${highlightCode(text, langForPath(path))}</code></pre>`;
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
  return sizePlaceholder(mode, file.size);
}

const sizePlaceholder = (mode, size) =>
  `<div class="fbinary">${mode === "toolarge" ? "file too large to preview" : "binary file"} · ${Number(size) || 0} bytes</div>`;

/**
 * renderFilesTab(body, { scope, callRpc }) — mount the browser into `body`.
 * `scope` is the plain server-resolved scope object ({task_id} / {project_id[,
 * worktree_id]}) spread into every fs.* call; `callRpc(method, params)` is the
 * app RPC (fs.* ride the app session, not the terminal socket). No polling —
 * fetches only on navigation/selection.
 */
export function renderFilesTab(body, { scope, callRpc }) {
  body.innerHTML = `<div class="files"><div class="ftree" id="ftree"></div><div class="fpreview" id="fpreview"><div class="empty">Select a file to preview.</div></div></div>`;
  const treeEl = body.querySelector("#ftree");
  const previewEl = body.querySelector("#fpreview");

  let dir = ""; // current directory, relative to the scope root
  let sourceOverride = false; // per-selected-file "view source" toggle

  const renderTree = (entries) => {
    treeEl.innerHTML = filesTreeHtml(dir, entries);
    if (dir) treeEl.querySelector(".fup").onclick = () => loadTree(parentPath(dir));
    treeEl.querySelectorAll(".fdir").forEach((row) => (row.onclick = () => loadTree(joinPath(dir, row.dataset.dir))));
    treeEl.querySelectorAll(".ffile").forEach((row) => (row.onclick = () => selectFile(joinPath(dir, row.dataset.file), row)));
  };

  const loadTree = async (nextDir) => {
    let res;
    try {
      res = await callRpc("fs.tree", { ...scope, path: nextDir });
    } catch (e) {
      treeEl.innerHTML = `<div class="empty">cannot list: ${esc((e && e.message) || "error")}</div>`;
      return;
    }
    dir = res.path || "";
    renderTree(res.entries || []);
  };

  const selectFile = async (path, row) => {
    treeEl.querySelectorAll(".frow.sel").forEach((r) => r.classList.remove("sel"));
    if (row) row.classList.add("sel");
    sourceOverride = false;
    previewEl.innerHTML = '<div class="empty">loading…</div>';
    let file;
    try {
      file = await callRpc("fs.read", { ...scope, path });
    } catch (e) {
      previewEl.innerHTML = `<div class="empty">cannot read: ${esc((e && e.message) || "error")}</div>`;
      return;
    }
    renderPreview(path, file);
  };

  const renderPreview = (path, file) => {
    const mode = previewModeFor(file.mime, file.truncated);
    const canToggle = previewHasSourceToggle(mode);
    const toggle = canToggle
      ? `<button class="btn mini" id="fsrctoggle">${sourceOverride ? "view rendered" : "view source"}</button>`
      : "";
    previewEl.innerHTML = `
      <div class="fphead"><span class="fppath mono">${esc(path)}</span><span class="fpsize mono">${Number(file.size) || 0} bytes</span>${toggle}</div>
      <div class="fpbody">${previewBodyHtml(path, file, sourceOverride)}</div>`;
    const toggleBtn = previewEl.querySelector("#fsrctoggle");
    if (toggleBtn)
      toggleBtn.onclick = () => {
        sourceOverride = !sourceOverride;
        renderPreview(path, file);
      };
  };

  loadTree("");
}

export { FS_READ_MAX_BYTES };
