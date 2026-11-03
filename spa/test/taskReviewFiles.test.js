// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { painted } from "./waits.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { readCached, wipeCache, writeCached } = await import("../src/core/localCache.js");
const { wipeUiRecords } = await import("../src/core/localUiStore.js");
const { cacheFileBody } = await import("../src/core/cacheLifetime.js");
const { armChangeEvents, dispatchChangeEvent, refetchEverything, resetChangeEvents } = await import("../src/core/changeEvents.js");
const { mountTaskReviewFiles } = await import("../src/core/taskReviewFiles.js");

const snapshot = { id: "snap-1" };
const git = { id: "dir-1", source_id: "source-1", is_git: true, status: "git", head: "abc" };
const live = { id: "dir-live", source_id: "source-live", is_git: false, status: "not_git" };
const textFile = (path, text) => ({ path, mime: "text/plain", size: text.length, content_b64: btoa(text), truncated: false, editable: false });
const tick = async () => { for (let i = 0; i < 8; i++) await new Promise((resolve) => setTimeout(resolve, 0)); };
const previewContains = (host, expected) => new Promise((resolve) => {
  const preview = host.querySelector(".trf-preview");
  if (preview.textContent.includes(expected)) return resolve();
  const observer = new MutationObserver(() => {
    if (!preview.textContent.includes(expected)) return;
    observer.disconnect();
    resolve();
  });
  observer.observe(preview, { childList: true, subtree: true, characterData: true });
});
const address = (directory, path, kind = "task-review-tree", snap = snapshot) => ({
  deviceId: "device", entityId: "project", kind,
  sub: JSON.stringify(["task", snap.id, directory.id, path, "head"]),
});

let hosts = [];
const mount = (directory, callRpc, options = {}) => {
  const host = document.createElement("div");
  document.body.append(host);
  const view = mountTaskReviewFiles(host, { deviceId: "device", projectId: "project", taskId: "task", workspaceId: "workspace", snapshot, directory, callRpc, ...options });
  hosts.push({ host, view });
  return { host, view };
};

beforeEach(async () => { await wipeCache(); await wipeUiRecords(); resetChangeEvents(); });
afterEach(() => { hosts.forEach(({ host, view }) => { view.dispose(); host.remove(); }); hosts = []; resetChangeEvents(); });

describe("task review Files", () => {
  it("lists the saved full head and opens unchanged blobs without filesystem RPC", async () => {
    const callRpc = vi.fn(async (method, params) => {
      expect(method).toBe("tasks.review.diff");
      expect(params).toMatchObject({ task_id: "task", snapshot_id: "snap-1", directory_id: "dir-1" });
      if (params.mode === "tree") return { path: params.path, entries: [{ name: "unchanged.txt", kind: "file", size: 9 }] };
      return textFile(params.path, "unchanged");
    });
    const { host, view } = mount(git, callRpc);
    await vi.waitFor(() => expect(host.querySelector('.frow[data-path="unchanged.txt"]')).toBeTruthy());
    await view.open("unchanged.txt");
    await vi.waitFor(() => expect(host.querySelector(".trf-preview")?.textContent).toContain("unchanged"));
    await vi.waitFor(async () => expect(await readCached(address(git, ""))).toMatchObject({ value: { entries: [{ name: "unchanged.txt" }] } }));
    await vi.waitFor(async () => expect(await readCached(address(git, "unchanged.txt", "task-review-file"))).toMatchObject({ value: { file: { path: "unchanged.txt" } } }));
  });

  it("keeps equal paths in separate directories and makes live reads exact", async () => {
    const callRpc = vi.fn(async (method, params) => {
      if (method === "fs.tree") return { path: params.path, entries: [{ name: "same.txt", kind: "file" }] };
      if (method === "fs.read") return textFile(params.path, params.source_id);
      throw new Error("unexpected RPC");
    });
    const other = { ...live, id: "dir-other", source_id: "source-other" };
    const first = mount(live, callRpc);
    const second = mount(other, callRpc);
    await first.view.open("same.txt");
    await second.view.open("same.txt");
    await painted(() => first.host.textContent.includes("dir-live") && second.host.textContent.includes("dir-other"));
    expect(first.host.textContent).toContain("dir-live");
    expect(second.host.textContent).toContain("dir-other");
    expect(callRpc.mock.calls.filter(([method]) => method.startsWith("fs.")).map(([, params]) => params.source_id)).toEqual(expect.arrayContaining(["dir-live", "dir-other"]));
    expect(await readCached(address(live, "same.txt", "task-review-live-file"))).toBeTruthy();
    expect(await readCached(address(other, "same.txt", "task-review-live-file"))).toBeTruthy();
  });

  it("shows failed refresh after cached paint and confines late reads to old keys", async () => {
    const callRpc = vi.fn(async (method, params) => {
      if (params.snapshot_id === "snap-1" && params.mode === "blob") return new Promise((resolve) => { callRpc.resolveOld = resolve; });
      if (params.mode === "tree") return { path: params.path, entries: [{ name: "same.txt", kind: "file" }] };
      if (params.mode === "blob") return textFile(params.path, "new body");
      throw new Error("Source unavailable");
    });
    const old = mount(git, callRpc);
    await old.view.open("same.txt");
    await tick();
    old.view.dispose();
    const next = mount(git, callRpc, { snapshot: { id: "snap-2" } });
    await next.view.open("same.txt");
    await tick();
    callRpc.resolveOld(textFile("same.txt", "old body"));
    await tick();
    expect(next.host.textContent).toContain("new body");
    expect(next.host.textContent).not.toContain("old body");
    expect(await readCached(address(git, "same.txt", "task-review-file"))).toBeTruthy();
    expect(await readCached(address(git, "same.txt", "task-review-file", { id: "snap-2" }))).toBeTruthy();
  });

  it("marks a missing live source unavailable while preserving cached rows and bytes", async () => {
    await writeCached(address(live, "", "task-review-live-tree"), { path: "", entries: [{ name: "same.txt", kind: "file" }] });
    await cacheFileBody({ ...address(live, "same.txt", "task-review-live-file"), kind: "task-review-live-file",
      path: address(live, "same.txt").sub, file: textFile("same.txt", "held bytes") });
    const callRpc = vi.fn(async () => { throw new Error("Source unavailable"); });
    const { host, view } = mount(live, callRpc);
    await view.open("same.txt");
    await tick();
    expect(host.textContent).toContain("held bytes");
    expect(host.querySelector('.frow[data-path="same.txt"]')).toBeTruthy();
    expect(host.textContent).toContain("Source unavailable");
    expect(callRpc.mock.calls.every(([, params]) => params.source_id === live.id)).toBe(true);
  });

  it("keeps a large review blob as pages at its own cache key", async () => {
    const callRpc = vi.fn(async (method, params) => {
      expect(method).toBe("tasks.review.diff");
      if (params.mode === "tree") return { path: params.path, entries: [{ name: "large.txt", kind: "file", size: 1_048_577 }] };
      if (!params.range) return { ...textFile(params.path, "first line\n"), size: 1_048_577, truncated: true };
      return { ...textFile(params.path, "first line\n"), size: 1_048_577,
        range: { offset: 0, end: 11, total: 11, version: "blob-oid" } };
    });
    const { host, view } = mount(git, callRpc);
    await view.open("large.txt");
    await vi.waitFor(async () => expect((await readCached(address(git, "large.txt", "task-review-file")))?.value.file).toMatchObject({ paged: true, of: "blob-oid" }));
    await vi.waitFor(() => expect(host.querySelector(".trf-preview")?.textContent).toContain("first line"));
    expect(callRpc.mock.calls.some(([, params]) => params.range?.offset === 0)).toBe(true);
    view.dispose();
    const remount = mount(git, vi.fn(async () => { throw new Error("offline"); }), { path: "large.txt" });
    await vi.waitFor(() => expect(remount.host.querySelector(".trf-preview")?.textContent).toContain("first line"));
    await vi.waitFor(() => expect(remount.host.querySelector(".trf-status")?.textContent).toContain("offline"));
  });

  it("shows an empty committed tree when a saved Git head has no commits", async () => {
    const unborn = { ...git, head: null, status: "no_commits" };
    const callRpc = vi.fn(async (method, params) => {
      expect(method).toBe("tasks.review.diff");
      expect(params.mode).toBe("tree");
      return { path: params.path, entries: [] };
    });
    const { host } = mount(unborn, callRpc);
    await tick();
    expect(host.textContent).toContain("Empty directory.");
    expect(callRpc).toHaveBeenCalledTimes(1);
  });

  it("offers accessible line comments anchored to the saved snapshot and directory", async () => {
    const onComment = vi.fn();
    const callRpc = vi.fn(async (_method, params) => params.mode === "tree"
      ? { path: "", entries: [{ name: "readme.txt", kind: "file" }] }
      : textFile(params.path, "first\nsecond\n"));
    const { host, view } = mount(git, callRpc, { onComment });
    await view.open("readme.txt");
    await tick();
    host.querySelector('[data-review-file-comment="2"]').click();
    expect(onComment).toHaveBeenCalledWith({ snapshot_id: "snap-1", directory_id: "dir-1", path: "readme.txt", side: "new", line: 2 });
    expect(host.querySelector('[data-review-file-comment="2"]').getAttribute("aria-label")).toContain("line 2");
  });

  it("paints inline comments and an anchor from cached bytes without waiting for a connection", async () => {
    await writeCached(address(git, ""), { path: "", entries: [{ name: "readme.txt", kind: "file" }] });
    await cacheFileBody({ ...address(git, "readme.txt", "task-review-file"), path: address(git, "readme.txt").sub,
      file: textFile("readme.txt", "first\nsecond\n") });
    const anchor = { snapshot_id: "snap-1", directory_id: git.id, path: "readme.txt", side: "new", line: 2 };
    const comments = [{ id: "inline", type: "comment", author: { kind: "user" }, anchor, body: "Keep **this line**." }];
    const callRpc = vi.fn(() => new Promise(() => {}));
    const { host, view } = mount(git, callRpc, { path: "readme.txt", comments, anchor });
    await vi.waitFor(() => expect(host.querySelector('[data-review-inline-comment="inline"]')).toBeTruthy());
    const line = host.querySelector('tr[data-new-line="2"]');
    expect(line.classList.contains("task-review-anchor")).toBe(true);
    expect(line.nextElementSibling.textContent).toContain("Keep this line.");
    expect(host.querySelector("[data-review-comment-line], [data-review-file-comment]")).toBeNull();
    expect(await view.reveal(anchor)).toBe(true);
  });

  it("shows range and saved-head commit comments in Files without claiming earlier commit lines", async () => {
    const anchor = { snapshot_id: snapshot.id, directory_id: git.id, path: "a.txt", side: "new", line: 1 };
    const comments = [
      { id: "range", anchor, body: "Range comment." },
      { id: "head", anchor: { ...anchor, commit: git.head }, body: "Head comment." },
      { id: "earlier", anchor: { ...anchor, commit: "earlier" }, body: "Earlier comment." },
    ];
    const callRpc = vi.fn(async (_method, params) => params.mode === "tree"
      ? { path: "", entries: [{ name: "a.txt", kind: "file" }] } : textFile(params.path, "line\n"));
    const { host, view } = mount(git, callRpc, { comments });
    await view.open("a.txt");
    await vi.waitFor(() => expect(host.querySelector('[data-review-inline-comment="range"]')).toBeTruthy());
    expect(host.querySelector('[data-review-inline-comment="head"]')).toBeTruthy();
    expect(host.querySelector('[data-review-inline-comment="earlier"]')).toBeNull();
  });

  it("reports tree and tab path selections for linkable URL state", async () => {
    const onSelectPath = vi.fn();
    const callRpc = vi.fn(async (_method, params) => params.mode === "tree"
      ? { path: "", entries: [{ name: "a.txt", kind: "file" }, { name: "b.txt", kind: "file" }] }
      : textFile(params.path, params.path));
    const { host, view } = mount(git, callRpc, { onSelectPath });
    await view.open("a.txt");
    await view.open("b.txt");
    await vi.waitFor(() => expect(onSelectPath).toHaveBeenLastCalledWith("b.txt"));
    host.querySelector('[data-tab-path="a.txt"]').click();
    await vi.waitFor(() => expect(onSelectPath).toHaveBeenLastCalledWith("a.txt"));
    view.dispose();
    const next = mount(git, callRpc, { onSelectPath });
    await tick();
    expect(next.host.querySelector(".trf-preview").textContent).not.toContain("a.txt");
  });

  it("keeps dotenv secrets masked while drawing their line comments", async () => {
    const anchor = { snapshot_id: "snap-1", directory_id: git.id, path: ".env", side: "new", line: 2 };
    const callRpc = vi.fn(async (_method, params) => params.mode === "tree"
      ? { path: "", entries: [{ name: ".env", kind: "file" }] }
      : textFile(params.path, "HOST=localhost\nPASSWORD=keep-private\n"));
    const { host, view } = mount(git, callRpc, { comments: [{ id: "secret", anchor, body: "Rotate this." }] });
    await view.open(".env");
    await vi.waitFor(() => expect(host.querySelector('[data-review-inline-comment="secret"]')).toBeTruthy());
    expect(host.querySelector(".trf-preview").innerHTML).not.toContain("keep-private");
    expect(host.querySelector(".trf-preview").textContent).toContain("●●●●●●●●●●");
    expect(host.querySelector('[data-new-line="2"]').nextElementSibling.textContent).toContain("Rotate this.");
  });

  it("uses source lines for anchored markdown so its comment opens beneath its line", async () => {
    const anchor = { snapshot_id: "snap-1", directory_id: git.id, path: "README.md", side: "new", line: 2 };
    const callRpc = vi.fn(async (_method, params) => params.mode === "tree"
      ? { path: "", entries: [{ name: "README.md", kind: "file" }] }
      : { ...textFile(params.path, "# Heading\nParagraph\n"), mime: "text/markdown" });
    const { host } = mount(git, callRpc, { path: "README.md", anchor,
      comments: [{ id: "markdown", anchor, body: "Use a concrete example." }] });
    await vi.waitFor(() => expect(host.querySelector('[data-review-inline-comment="markdown"]')).toBeTruthy());
    expect(host.querySelector('tr[data-new-line="2"]').classList.contains("task-review-anchor")).toBe(true);
    expect(host.querySelector(".trf-preview h1")).toBeNull();
  });

  it("loads cached paged source through the target line when an anchor opens", async () => {
    const { writeBodyPages, bytePagesOf } = await import("../src/core/bodyPages.js");
    const fileHead = address(git, "large.txt", "task-review-file");
    const body = "one\ntwo\nthree\nfour\n";
    await writeCached(fileHead, { file: { path: "large.txt", mime: "text/plain", size: body.length, paged: true, of: "version" } });
    const pages = bytePagesOf(btoa(body), { of: "version", bytes: 8 });
    await writeBodyPages(fileHead, [pages[0]]);
    const anchor = { snapshot_id: snapshot.id, directory_id: git.id, path: "large.txt", side: "new", line: 4 };
    const callRpc = vi.fn(async (_method, params) => {
      if (params.mode === "tree") return { path: "", entries: [{ name: "large.txt", kind: "file" }] };
      if (!params.range) return new Promise(() => {});
      const page = pages.find((part) => part.offset === params.range.offset);
      return { ...textFile(params.path, atob(page.body)), range: { offset: page.offset, end: page.end, total: page.total, version: "version" } };
    });
    const { host } = mount(git, callRpc, { path: "large.txt", anchor, comments: [{ id: "last", anchor, body: "Last line." }] });
    await vi.waitFor(() => expect(host.querySelector('[data-new-line="4"]')?.classList.contains("task-review-anchor")).toBe(true));
    expect(host.querySelector('[data-new-line="4"]').nextElementSibling.textContent).toContain("Last line.");
  });

  it("keeps a file failure visible when the tree refresh succeeds", async () => {
    let failFile = false;
    let finishTree;
    const callRpc = vi.fn(async (method, params) => {
      if (method === "fs.tree" && failFile) return new Promise((resolve) => { finishTree = () => resolve({ path: params.path, entries: [{ name: "same.txt", kind: "file" }] }); });
      if (method === "fs.tree") return { path: params.path, entries: [{ name: "same.txt", kind: "file" }] };
      if (failFile) throw new Error("File unavailable");
      return textFile(params.path, "held");
    });
    const { host, view } = mount(live, callRpc);
    await view.open("same.txt");
    await tick();
    failFile = true;
    view.refresh();
    await tick();
    expect(host.querySelector(".trf-status").textContent).toContain("File unavailable");
    finishTree();
    await tick();
    expect(host.querySelector(".trf-status").textContent).toContain("File unavailable");
    expect(host.querySelector(".trf-preview").textContent).toContain("held");
  });

  it("refetches named live files on push and on reconnect, without touching saved Git", async () => {
    let text = "first";
    let answerThirdRead;
    const thirdRead = new Promise((resolve) => { answerThirdRead = resolve; });
    let announceThirdRead;
    const thirdReadStarted = new Promise((resolve) => { announceThirdRead = resolve; });
    const callRpc = vi.fn(async (method, params) => {
      if (method === "fs.tree") return { path: params.path, entries: [{ name: "same.txt", kind: "file" }] };
      if (text === "third") { announceThirdRead(); return thirdRead; }
      return textFile(params.path, text);
    });
    const { host, view } = mount(live, callRpc);
    let announceGitRead;
    const gitReadStarted = new Promise((resolve) => { announceGitRead = resolve; });
    const gitRpc = vi.fn(async (_method, params) => { announceGitRead(); return { path: params.path, entries: [] }; });
    mount(git, gitRpc);
    const firstPaint = previewContains(host, "first");
    await view.open("same.txt");
    await firstPaint;
    await gitReadStarted;
    const gitBefore = gitRpc.mock.calls.length;
    const before = callRpc.mock.calls.filter(([method]) => method === "fs.read").length;
    armChangeEvents({ push_events: true }, "device");
    dispatchChangeEvent({ type: "changes", items: [{ entity_id: "workspace", files: { paths: ["other.txt"] } }] }, "device");
    await tick();
    expect(callRpc.mock.calls.filter(([method]) => method === "fs.read")).toHaveLength(before);
    text = "second";
    const secondPaint = previewContains(host, "second");
    dispatchChangeEvent({ type: "changes", items: [{ entity_id: "workspace", files: { paths: ["same.txt"] } }] }, "device");
    await secondPaint;
    expect(host.querySelector(".trf-preview").textContent).toContain("second");
    text = "third";
    const thirdPaint = previewContains(host, "third");
    refetchEverything("device");
    await thirdReadStarted;
    expect(host.querySelector(".trf-preview").textContent).toContain("second");
    answerThirdRead(textFile("same.txt", "third"));
    await thirdPaint;
    expect(host.querySelector(".trf-preview").textContent).toContain("third");
    expect(gitRpc).toHaveBeenCalledTimes(gitBefore);
  });
});
