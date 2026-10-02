// The task review's read-only Files browser. Git reads come from the saved
// head; plain folders are expressly live and use the saved directory's ID.
import "./taskReviewFiles.css";
import { cacheFileBody } from "./cacheLifetime.js";
import { pageFromAnswer } from "./bodyPages.js";
import { mountFileTree } from "./fileTree.js";
import { mountFileTabs } from "./fileTabs.js";
import { fileBodyReading, isMediaPath } from "./fileViewer.js";
import { mountPagedFile, sourceLinesPainter, wholeBytesPainter, wholeTextPainter } from "./pagedFileView.js";
import { langForPath } from "./highlight.js";
import { markdownHtml } from "./markdown.js";
import { attachMediaSource, createMediaBody, releaseMediaSource } from "./mediaBlob.js";
import { readCached, recordWriteOf, subscribeCache, writeCached } from "./localCache.js";
import { esc } from "./text.js";
import { decodeBase64Text, mediaPreviewHtml, previewModeFor, sourcePreviewHtml } from "../views/files.js";
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

const previewHtml = (path, file) => {
  const head = `<div class="trf-file-head"><span class="mono">${esc(path)}</span><span>${Number(file.size) || 0} bytes</span></div>`;
  if (file.paged) return head + '<div class="trf-paged"></div>';
  return head + previewBodyHtml(path, file, previewModeFor(file.mime, file.truncated));
};

const pagedPainter = (path, file) => {
  const mode = previewModeFor(file.mime, file.truncated);
  if (isDotenvPath(path)) return wholeTextPainter((host, text) => { host.innerHTML = renderDotenvSourceHtml(text).html; });
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
  const failureAddress = address("task-review-error", "");
  const paintFailure = async () => {
    const held = await readCached(failureAddress);
    if (!disposed) status.textContent = held?.value?.message || "";
  };
  const unwatchFailure = subscribeCache(failureAddress, () => void paintFailure());
  void paintFailure();

  const setFailure = (message) => {
    void writeCached(failureAddress, { message });
  };
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
    preview.innerHTML = previewHtml(name, file);
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
      setFailure("");
      await paintHeldFile(name, serial);
    } catch (error) {
      if (!disposed && selected === name && serial === fileSerial) setFailure(readError(error));
    }
  };

  const showFile = (name) => {
    selected = name;
    const serial = ++fileSerial;
    unwatchFile?.();
    unwatchFile = null;
    paged?.dispose();
    paged = null;
    tree.setOpenPath(name);
    if (!name) { preview.innerHTML = '<div class="fpidle">Choose a file</div>'; return; }
    if (!safePath(name)) { setFailure("Invalid file path"); return; }
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
      try { const listing = await listDirectory(name); setFailure(""); return listing; }
      catch (error) { setFailure(readError(error)); throw error; }
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
  if (sourceUnavailable) setFailure("Source unavailable");

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
      unwatchFailure();
      paged?.dispose();
      preview.querySelectorAll("img, audio, video").forEach(releaseMediaSource);
      tabs.dispose();
      tree.dispose();
    },
  };
}
