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
import { isDotenvPath, maskDotenvLine, SPOILER_DOTS } from "./secrets.js";
import { paintInlineReviewRows, placeReviewComposer, reviewComposerMatches, reviewReplyAt } from "./taskReviewInline.js";

const treeKind = (live) => live ? "task-review-live-tree" : "task-review-tree";
const fileKind = (live) => live ? "task-review-live-file" : "task-review-file";
const imageModes = new Set(["image", "svg"]);
const avModes = new Set(["audio", "video"]);
const readError = (error) => error?.message || "Source unavailable";
const safePath = (path) => typeof path === "string" && path.length > 0 && !path.startsWith("/")
  && path.split("/").every((part) => part && part !== "." && part !== "..");
const maskedSource = (text) => text.split("\n").map((line) => {
  const masked = maskDotenvLine(line);
  return masked.masked ? `${masked.prefix}${SPOILER_DOTS}${masked.suffix}` : line;
}).join("\n");
const sourceHtml = (path, text) => sourcePreviewHtml(path, isDotenvPath(path) ? maskedSource(text) : text);

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
  const source = sourceHtml(path, text);
  return source + (file.truncated ? '<div class="ftrunc">truncated</div>' : "");
};

const previewHtml = (path, file, showSource) => {
  const head = `<div class="trf-file-head"><span class="mono">${esc(path)}</span><span>${Number(file.size) || 0} bytes</span></div>`;
  if (file.paged) return head + '<div class="trf-paged"></div>';
  const mode = showSource ? "source" : previewModeFor(file.mime, file.truncated);
  return head + previewBodyHtml(path, file, mode);
};

const pagedPainter = (path, file, showSource) => {
  const mode = showSource ? "source" : previewModeFor(file.mime, file.truncated);
  if (isDotenvPath(path)) return wholeBytesPainter((host, body) => {
    host.innerHTML = sourceHtml(path, decodeBase64Text(body));
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
export function mountTaskReviewFiles(host, options) {
  let { comments = [] } = options;
  const { deviceId, projectId, taskId, workspaceId, snapshot, directory, callRpc, path, onComment, onReply, onPaint = () => {},
    anchor, onSelectPath, identities, keepReadingPlace = (paint) => paint() } = {
    path: "", onComment: null, anchor: null, onSelectPath: null, identities: {}, keepReadingPlace: (paint) => paint(), ...options,
  };
  const live = !directory.is_git;
  const identity = { deviceId, projectId, taskId, snapshot, directory };
  const address = (kind, name) => taskReviewFileAddress(identity, kind, name);
  const sourceUnavailable = directory.status === "unavailable";
  let disposed = false;
  let selected = null;
  let fileSerial = 0;
  let unwatchFile = null;
  let paged = null;
  let selectedAnchor = anchor;
  let anchorScrolled = false;
  let anchorSeeking = null;
  let selectionGeneration = 0;
  let tabSelection = null;
  let shownFile = null;
  let shownSource = false;
  const navigationCurrent = (generation) => !disposed && generation === selectionGeneration;
  const beginSelection = (target = null) => {
    selectedAnchor = target;
    anchorScrolled = false;
    return ++selectionGeneration;
  };

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
    if (!disposed && serial === failureSerial) keepReadingPlace(() => {
      status.textContent = fileHeld?.value?.message || treeHeld?.value?.message || "";
    });
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

  const anchoredRow = () => {
    if (!selectedAnchor || selectedAnchor.path !== selected || selectedAnchor.side !== "new") return null;
    return [...preview.querySelectorAll("tr[data-new-line]")]
      .find((row) => Number(row.dataset.newLine) === Number(selectedAnchor.line)) || null;
  };
  const paintAnchor = () => {
    preview.querySelectorAll(".task-review-anchor").forEach((row) => row.classList.remove("task-review-anchor"));
    const row = anchoredRow();
    if (!row) return false;
    row.classList.add("task-review-anchor");
    if (!anchorScrolled) row.scrollIntoView?.({ block: "center" });
    anchorScrolled = true;
    return true;
  };
  const anchorIsSelected = () => Boolean(selectedAnchor) && selectedAnchor.path === selected;
  const seekAnchor = async (generation = selectionGeneration) => {
    if (anchorSeeking === generation || !anchorIsSelected()) return;
    anchorSeeking = generation;
    try {
      for (let page = 0; page < 32 && paged; page++) {
        if (!navigationCurrent(generation) || paintAnchor()) break;
        if (!(await paged.more())) break;
      }
    } finally { if (anchorSeeking === generation) anchorSeeking = null; }
  };
  const paintLineMetadata = () => {
    wireCommentLines(preview);
    paintInlineReviewRows(preview, comments, {
      snapshotId: snapshot.id, directoryId: directory.id, path: selected, deviceId, projectId, identities,
      allowedCommits: ["", directory.head], replyable: Boolean(onReply),
    });
    if (!paintAnchor()) void seekAnchor();
    onPaint();
  };
  const showSourceFor = (name, file) => {
    if (!["markdown", "html", "svg"].includes(previewModeFor(file.mime, file.truncated))) return false;
    if (selectedAnchor?.path === name) return true;
    return comments.some((row) => row.anchor?.snapshot_id === snapshot.id
      && row.anchor?.directory_id === directory.id && row.anchor?.path === name);
  };

  const paintFile = (name, file) => keepReadingPlace(() => {
    if (disposed || selected !== name) return;
    paged?.dispose();
    paged = null;
    preview.querySelectorAll("img, audio, video").forEach(releaseMediaSource);
    const showSource = showSourceFor(name, file);
    shownFile = file;
    shownSource = showSource;
    preview.innerHTML = previewHtml(name, file, showSource);
    paintLineMetadata();
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
      painter: pagedPainter(name, file, showSource),
      onPaint: paintLineMetadata, keepReadingPlace,
    });
  });

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

  const fileNeedsRefresh = (held, failure) => live || !held?.value?.file || Boolean(failure?.value?.message);
  const readShownFile = async (name, serial) => {
    const held = await paintHeldFile(name, serial);
    const failure = await readCached(fileFailureAddress(name));
    if (disposed || selected !== name || serial !== fileSerial) return;
    if (fileNeedsRefresh(held, failure)) void refreshFile(name);
  };
  const acceptTabSelection = (name) => {
    if (tabSelection?.name === name) return navigationCurrent(tabSelection.generation);
    beginSelection();
    return !disposed;
  };
  const showFile = (name) => {
    if (!acceptTabSelection(name)) return;
    keepReadingPlace(() => {
      selected = name;
      onSelectPath?.(name || "");
      const serial = ++fileSerial;
      shownFile = null;
      shownSource = false;
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
      void readShownFile(name, serial);
    });
  };

  const tree = mountFileTree(treeEl, {
    listingAddress: (name) => address(treeKind(live), name),
    stateAddress: address("ui-task-review-files", "expanded"),
    readsForItself: () => live,
    keepHeldOnError: true, keepReadingPlace,
    listDirectory: async (name) => {
      try { const listing = await listDirectory(name); setTreeFailure(""); return listing; }
      catch (error) { setTreeFailure(readError(error)); throw error; }
    },
    finePointer: () => window.matchMedia?.("(pointer: fine)").matches === true,
    onOpen: (name) => void open(name),
  });
  const tabs = mountFileTabs(host.querySelector(".trf-tabs"), {
    stateAddress: null,
    dirtyPaths: () => new Set(), confirmClose: async () => true, onClose: () => {}, onShow: showFile,
    keepReadingPlace,
  });
  if (sourceUnavailable) setTreeFailure("Source unavailable");
  const onTabSelection = (event) => {
    if (event.target.closest("[data-tab-path], [data-tab-close]")) beginSelection();
  };
  host.querySelector(".trf-tabs").addEventListener("click", onTabSelection, true);
  const selectTab = async (name, generation) => {
    await tabs.ready;
    if (!navigationCurrent(generation)) return false;
    tabSelection = { name, generation };
    try { await tabs.open(name); }
    finally { if (tabSelection?.generation === generation) tabSelection = null; }
    return navigationCurrent(generation);
  };
  const open = async (name) => {
    if (!safePath(name) || disposed) return;
    const generation = beginSelection();
    await tree.reveal(name);
    if (navigationCurrent(generation)) await selectTab(name, generation);
  };

  const onPreviewClick = (event) => {
    if (reviewReplyAt(event, comments, onReply, (row) => ({ snapshot_id: snapshot.id, directory_id: directory.id, path: selected, side: "new", line: Number(row?.dataset.newLine) }))) return;
    const line = event.target.closest?.("[data-review-file-comment]");
    if (line) commentAt(Number(line.dataset.reviewFileComment));
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

  const reveal = async (target) => {
    if (!safePath(target?.path) || disposed) return false;
    const generation = beginSelection(target);
    await tree.reveal(target.path);
    if (!navigationCurrent(generation) || !(await selectTab(target.path, generation))) return false;
    await paintHeldFile(target.path, fileSerial);
    if (!navigationCurrent(generation)) return false;
    if (paintAnchor()) return true;
    await seekAnchor(generation);
    return navigationCurrent(generation) && paintAnchor();
  };
  if (anchor?.path) void reveal(anchor);
  else if (safePath(path)) void open(path);

  return {
    reveal,
    placeComposer: (row, anchor) => reviewComposerMatches(anchor, { snapshotId: snapshot.id, directoryId: directory.id, allowedCommits: ["", directory.head] })
      && anchor.path === selected && placeReviewComposer(preview, row, anchor),
    updateComments(next) {
      comments = next || [];
      if (disposed) return;
      if (shownFile && !shownSource && showSourceFor(selected, shownFile)) paintFile(selected, shownFile);
      else keepReadingPlace(paintLineMetadata);
    },
    open,
    refresh() {
      if (disposed) return;
      tree.relist();
      if (selected) void refreshFile(selected);
    },
    dispose() {
      disposed = true;
      selectionGeneration++;
      fileSerial += 1;
      unwatchFile?.();
      unwatchTreeFailure();
      unwatchFileFailure();
      liveWatcher?.dispose();
      preview.removeEventListener("click", onPreviewClick);
      host.querySelector(".trf-tabs").removeEventListener("click", onTabSelection, true);
      paged?.dispose();
      preview.querySelectorAll("img, audio, video").forEach(releaseMediaSource);
      tabs.dispose();
      tree.dispose();
    },
  };
}
