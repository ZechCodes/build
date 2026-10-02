// The task review's read-only Files browser. Git reads come from the saved
// head; plain folders are expressly live and use the saved directory's ID.
import "./taskReviewFiles.css";
import { cacheFileBody } from "./cacheLifetime.js";
import { pageFromAnswer } from "./bodyPages.js";
import { mountFileTree } from "./fileTree.js";
import { mountFileTabs } from "./fileTabs.js";
import { fileBodyReading, isMediaPath } from "./fileViewer.js";
import { mountPagedFile, sourceLinesPainter, wholeBytesPainter } from "./pagedFileView.js";
import { langForPath } from "./highlight.js";
import { markdownHtml } from "./markdown.js";
import { attachMediaSource, createMediaBody, releaseMediaSource } from "./mediaBlob.js";
import { readCached, recordWriteOf, subscribeCache, writeCached } from "./localCache.js";
import { watchChanges } from "./changeEvents.js";
import { esc } from "./text.js";
import { decodeBase64Text, mediaPreviewHtml, previewModeFor, sourcePreviewHtml } from "./filePreview.js";
import { isDotenvPath, renderDotenvSourceHtml } from "./secrets.js";

const treeKind = (live) => live ? "task-review-live-tree" : "task-review-tree";
const fileKind = (live) => live ? "task-review-live-file" : "task-review-file";
const imageModes = new Set(["image", "svg"]);
const avModes = new Set(["audio", "video"]);
const readError = (error) => error?.message || "Source unavailable";
const safePath = (path) => typeof path === "string" && path.length > 0 && !path.startsWith("/")
  && path.split("/").every((part) => part && part !== "." && part !== "..");

/** The saved directory and snapshot own every address, including live reads. */
export const taskReviewFileAddress = ({ deviceId, projectId, taskId, snapshot, directory }, kind, path = "") => ({
  deviceId, entityId: projectId, kind,
  sub: JSON.stringify([taskId, snapshot.id, directory.id, path, "head"]),
});

const readParams = (taskId, snapshot, directory, mode, path, range) => ({
  task_id: taskId, snapshot_id: snapshot.id, directory_id: directory.id, mode, path,
  ...(range ? { range } : {}),
});

const liveParams = (workspaceId, directory, path, range) => ({
  workspace_id: workspaceId, source_id: directory.id, path,
  ...(range ? { range } : {}),
});

const previewBodyHtml = (path, file, mode) => {
  if (mode === "binary" || mode === "toolarge") return '<div class="fbinary">Binary file</div>';
  if (imageModes.has(mode)) return '<img class="trf-media" alt="">';
  if (avModes.has(mode)) return mediaPreviewHtml(mode);
  if (mode === "html") return `<iframe class="fhtml" sandbox="" src="data:text/html;base64,${file.content_b64}"></iframe>`;
  const text = decodeBase64Text(file.content_b64);
  if (mode === "markdown") return `<div class="plan">${markdownHtml(text)}</div>`;
  const source = isDotenvPath(path) ? renderDotenvSourceHtml(text).html : sourcePreviewHtml(path, text);
  return source + (file.truncated ? '<div class="ftrunc">truncated</div>' : "");
};

const previewHtml = (path, file, commentable) => {
  const lineAction = commentable ? '<label>Line <input data-review-line-input type="number" min="1" value="1" inputmode="numeric" aria-label="Line to comment on"></label><button type="button" data-review-comment-line>Comment</button>' : "";
  const head = `<div class="trf-file-head"><span class="mono">${esc(path)}</span><span>${Number(file.size) || 0} bytes</span>${lineAction}</div>`;
  if (file.paged) return head + '<div class="trf-paged"></div>';
  return head + previewBodyHtml(path, file, previewModeFor(file.mime, file.truncated));
};

const pagedPainter = (path, file) => {
  const mode = previewModeFor(file.mime, file.truncated);
  if (isDotenvPath(path)) return wholeBytesPainter((host, body) => {
    host.innerHTML = renderDotenvSourceHtml(decodeBase64Text(body)).html;
  });
  if (imageModes.has(mode) || avModes.has(mode)) {
    return wholeBytesPainter((host, pages) => {
      host.innerHTML = imageModes.has(mode) ? '<img class="trf-media" alt="">' : mediaPreviewHtml(mode);
      attachMediaSource(host.querySelector("img, audio, video"), createMediaBody(pages, file.mime));
    }, { asPages: true });
  }
  if (mode === "html") return wholeBytesPainter((host, body) => {
    host.innerHTML = `<iframe class="fhtml" sandbox="" src="data:text/html;base64,${body}"></iframe>`;
  });
  return sourceLinesPainter(langForPath(path));
};

/**
 * Mount a saved review directory's Files view. The returned open(path) accepts
 * a path from Changes; refresh() retries the currently shown tree and file.
 */
export function mountTaskReviewFiles(host, { deviceId, projectId, taskId, workspaceId, snapshot, directory, callRpc, path = "", onComment = null }) {
  const live = !directory.is_git;
  const identity = { deviceId, projectId, taskId, snapshot, directory };
  const address = (kind, name) => taskReviewFileAddress(identity, kind, name);
  const sourceUnavailable = directory.status === "unavailable";
  let disposed = false;
  let selected = null;
  let fileSerial = 0;
  let unwatchFile = null;
  let paged = null;

  host.classList.add("task-review-files");
  host.innerHTML = `<div class="trf-labels">${live ? '<span>Not a Git repository</span><span>Live files — not saved with this review</span>' : '<span>Files at saved commit</span>'}</div><div class="trf-status" role="status"></div><div class="trf-layout"><div class="trf-tree" role="tree" aria-label="Review files"></div><div class="trf-right"><div class="trf-tabs"></div><div class="trf-preview"></div></div></div>`;
  const status = host.querySelector(".trf-status");
  const preview = host.querySelector(".trf-preview");
  const treeEl = host.querySelector(".trf-tree");
  const treeFailureAddress = address("task-review-tree-error", "");
  const fileFailureAddress = (name) => address("task-review-file-error", name);
  let failureSerial = 0;
  const paintFailure = async () => {
    const serial = ++failureSerial;
    const path = selected;
    const [treeHeld, fileHeld] = await Promise.all([
      readCached(treeFailureAddress), path ? readCached(fileFailureAddress(path)) : null,
    ]);
    if (!disposed && serial === failureSerial) status.textContent = fileHeld?.value?.message || treeHeld?.value?.message || "";
  };
  const unwatchTreeFailure = subscribeCache(treeFailureAddress, () => void paintFailure());
  const unwatchFileFailure = subscribeCache({ deviceId, entityId: projectId, kind: "task-review-file-error" }, () => void paintFailure());
  void paintFailure();

  const setTreeFailure = (message) => void writeCached(treeFailureAddress, { message });
  const setFileFailure = (name, message) => void writeCached(fileFailureAddress(name), { message });
  const listDirectory = (name) => {
    if (sourceUnavailable) throw new Error("Source unavailable");
    return live
      ? callRpc("fs.tree", liveParams(workspaceId, directory, name))
      : callRpc("tasks.review.diff", readParams(taskId, snapshot, directory, "tree", name));
  };
  const readFile = (name, range) => {
    if (sourceUnavailable) throw new Error("Source unavailable");
    return live
      ? callRpc("fs.read", liveParams(workspaceId, directory, name, range))
      : callRpc("tasks.review.diff", readParams(taskId, snapshot, directory, "blob", name, range));
  };
  const pageReader = (name, mime) => async (offset, bytes) => {
    const raw = isMediaPath(name) && fileBodyReading(mime) === "media";
    return pageFromAnswer(await readFile(name, { offset, bytes, ...(raw ? { raw: true } : {}) }), "content_b64");
  };

  const paintFile = (name, file) => {
    if (disposed || selected !== name) return;
    paged?.dispose();
    paged = null;
    preview.querySelectorAll("img, audio, video").forEach(releaseMediaSource);
    preview.innerHTML = previewHtml(name, file, Boolean(onComment));
    wireCommentLines(preview);
    if (!file.paged) {
      const media = preview.querySelector("img.trf-media, audio.fmedia, video.fmedia");
      if (media) attachMediaSource(media, [file.content_b64], file.mime);
      return;
    }
    const body = preview.querySelector(".trf-paged");
    paged = mountPagedFile(body, {
      head: address(fileKind(live), name), file,
      readPage: pageReader(name, file.mime),
      restart: () => void refreshFile(name),
      painter: pagedPainter(name, file),
      onPaint: () => wireCommentLines(preview),
    });
  };

  const commentAt = (line) => {
    if (!onComment || !selected || !Number.isSafeInteger(line) || line < 1) return;
    onComment({ snapshot_id: snapshot.id, directory_id: directory.id, path: selected, side: "new", line });
  };
  const wireCommentLines = (root) => {
    if (!onComment) return;
    root.querySelectorAll("tr[data-new-line] td.fsrc-ln").forEach((cell) => {
      if (cell.querySelector("button")) return;
      const line = Number(cell.parentElement.dataset.newLine);
      const button = document.createElement("button");
      button.type = "button";
      button.dataset.reviewFileComment = String(line);
      button.setAttribute("aria-label", `Comment on line ${line}`);
      button.textContent = String(line);
      cell.replaceChildren(button);
    });
  };

  const paintHeldFile = async (name, serial) => {
    const record = await readCached(address(fileKind(live), name));
    if (disposed || selected !== name || serial !== fileSerial) return record;
    if (record?.value?.file) paintFile(name, record.value.file);
    return record;
  };

  const refreshFile = async (name) => {
    const serial = fileSerial;
    const head = address(fileKind(live), name);
    const previous = await readCached(head);
    try {
      const file = await readFile(name);
      const written = recordWriteOf(previous);
      await cacheFileBody({ deviceId, entityId: projectId, kind: head.kind, path: head.sub, file,
        written, readPage: pageReader(name, file.mime) });
      if (disposed || selected !== name || serial !== fileSerial) return;
      setFileFailure(name, "");
      await paintHeldFile(name, serial);
    } catch (error) {
      if (!disposed && selected === name && serial === fileSerial) setFileFailure(name, readError(error));
    }
  };

  const showFile = (name) => {
    selected = name;
    const serial = ++fileSerial;
    void paintFailure();
    unwatchFile?.();
    unwatchFile = null;
    paged?.dispose();
    paged = null;
    tree.setOpenPath(name);
    if (!name) { preview.innerHTML = '<div class="fpidle">Choose a file</div>'; return; }
    if (!safePath(name)) { setFileFailure(name, "Invalid file path"); return; }
    preview.innerHTML = '<div class="throbber" role="status" aria-label="loading"></div>';
    unwatchFile = subscribeCache(address(fileKind(live), name), () => void paintHeldFile(name, serial));
    void paintHeldFile(name, serial).then(() => {
      if (disposed || selected !== name || serial !== fileSerial) return;
      void refreshFile(name);
    });
  };

  const tree = mountFileTree(treeEl, {
    listingAddress: (name) => address(treeKind(live), name),
    stateAddress: address("ui-task-review-files", "expanded"),
    readsForItself: () => true,
    keepHeldOnError: true,
    listDirectory: async (name) => {
      try { const listing = await listDirectory(name); setTreeFailure(""); return listing; }
      catch (error) { setTreeFailure(readError(error)); throw error; }
    },
    finePointer: () => window.matchMedia?.("(pointer: fine)").matches === true,
    onOpen: (name) => void tabs.open(name),
  });
  const tabs = mountFileTabs(host.querySelector(".trf-tabs"), {
    stateAddress: address("ui-task-review-files", "tabs"),
    dirtyPaths: () => new Set(), confirmClose: async () => true, onClose: () => {}, onShow: showFile,
    initial: safePath(path) ? path : null,
  });
  if (safePath(path)) void tree.reveal(path);
  if (sourceUnavailable) setTreeFailure("Source unavailable");

  const onPreviewClick = (event) => {
    const line = event.target.closest?.("[data-review-file-comment]");
    if (line) commentAt(Number(line.dataset.reviewFileComment));
    const target = event.target.closest?.("[data-review-comment-line]");
    if (target) commentAt(Number(preview.querySelector("[data-review-line-input]")?.value));
  };
  preview.addEventListener("click", onPreviewClick);

  const refreshLive = () => {
    if (disposed) return;
    tree.relist();
    if (selected) void refreshFile(selected);
  };
  const liveWatcher = live ? watchChanges({
    deviceId, entity: workspaceId, kinds: ["files"], mode: "realtime",
    refresh: refreshLive,
    onChanges: (items) => {
      if (items.some((item) => !("files" in item))) return refreshLive();
      const files = items.filter((item) => !item.source_id || item.source_id === directory.id)
        .map((item) => item.files).filter((item) => item && (!item.source_id || item.source_id === directory.id));
      if (!files.length) return;
      if (files.some((item) => item.truncated || !Array.isArray(item.paths))) return refreshLive();
      const paths = files.flatMap((item) => item.paths);
      if (paths.length || files.some((item) => item.root)) tree.relist(paths);
      if (selected && paths.includes(selected)) void refreshFile(selected);
    },
  }) : null;

  return {
    async open(name) {
      if (!safePath(name) || disposed) return;
      await tree.reveal(name);
      await tabs.open(name);
    },
    refresh() {
      if (disposed) return;
      tree.relist();
      if (selected) void refreshFile(selected);
    },
    dispose() {
      disposed = true;
      fileSerial += 1;
      unwatchFile?.();
      unwatchTreeFailure();
      unwatchFileFailure();
      liveWatcher?.dispose();
      preview.removeEventListener("click", onPreviewClick);
      paged?.dispose();
      preview.querySelectorAll("img, audio, video").forEach(releaseMediaSource);
      tabs.dispose();
      tree.dispose();
    },
  };
}
