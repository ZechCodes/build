// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const delayed = vi.hoisted(() => new Map());
vi.mock("../src/core/fileTree.js", async (original) => {
  const treeModule = await original();
  return { ...treeModule, mountFileTree: (...args) => {
    const tree = treeModule.mountFileTree(...args);
    return { ...tree, reveal: async (path) => {
      await delayed.get(path)?.promise;
      return tree.reveal(path);
    } };
  } };
});

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const { wipeCache } = await import("../src/core/localCache.js");
const { wipeUiRecords } = await import("../src/core/localUiStore.js");
const { mountTaskReviewFiles } = await import("../src/core/taskReviewFiles.js");

const snapshot = { id: "revision" };
const directory = { id: "directory", is_git: true, status: "git", head: "head" };
const anchor = (path) => ({ snapshot_id: snapshot.id, directory_id: directory.id, path, side: "new", line: 1 });
const defer = (path) => {
  let release;
  const promise = new Promise((resolve) => { release = resolve; });
  delayed.set(path, { promise });
  return release;
};
const body = (path) => ({ path, mime: "text/plain", size: path.length, content_b64: btoa(path), truncated: false });
let host, view, onSelectPath;
beforeEach(async () => { await wipeCache(); await wipeUiRecords(); delayed.clear(); });
afterEach(() => { view?.dispose(); host?.remove(); });

function mount(fileOf = body) {
  host = document.createElement("div");
  document.body.append(host);
  onSelectPath = vi.fn();
  view = mountTaskReviewFiles(host, { deviceId: "device", projectId: "project", taskId: "task", snapshot, directory,
    onSelectPath, callRpc: async (_method, params) => params.mode === "tree"
      ? { path: params.path, entries: [{ name: "b.txt", kind: "file" }] } : fileOf(params.path),
  });
}
const showing = async (path) => vi.waitFor(() => expect(host.querySelector(".trf-preview").textContent).toContain(path));

describe("review Files navigation ownership", () => {
  it("keeps B selected when deferred directory reveals finish B then A", async () => {
    mount();
    const releaseA = defer("a/one.txt");
    const releaseB = defer("b/two.txt");
    const first = view.open("a/one.txt");
    const second = view.open("b/two.txt");
    releaseB();
    await second;
    await showing("b/two.txt");
    releaseA();
    await first;
    expect(onSelectPath).toHaveBeenLastCalledWith("b/two.txt");
    expect(host.querySelector(".trf-preview").textContent).not.toContain("a/one.txt");
  });

  it("invalidates a pending directory open when the user selects an already active tab", async () => {
    mount();
    await view.open("b.txt");
    await showing("b.txt");
    const releaseA = defer("a/one.txt");
    const first = view.open("a/one.txt");
    host.querySelector('[data-tab-path="b.txt"]').click();
    releaseA();
    await first;
    expect(onSelectPath).toHaveBeenLastCalledWith("b.txt");
    expect(host.querySelector(".trf-preview").textContent).toContain("b.txt");
  });

  it("does not restore an older anchor after a newer Files open", async () => {
    mount();
    const releaseA = defer("a/one.txt");
    const first = view.reveal(anchor("a/one.txt"));
    await view.open("b.txt");
    await showing("b.txt");
    releaseA();
    await first;
    expect(onSelectPath).toHaveBeenLastCalledWith("b.txt");
    expect(host.querySelector(".task-review-anchor")).toBeNull();
  });

  it("keeps the later anchor when two directory reveals resolve in reverse order", async () => {
    mount();
    const releaseA = defer("a/one.txt");
    const releaseB = defer("b/two.txt");
    const first = view.reveal(anchor("a/one.txt"));
    const second = view.reveal(anchor("b/two.txt"));
    releaseB();
    await second;
    await showing("b/two.txt");
    releaseA();
    expect(await first).toBe(false);
    expect(onSelectPath).toHaveBeenLastCalledWith("b/two.txt");
    expect(host.querySelector(".task-review-anchor").textContent).toContain("b/two.txt");
  });

  it("does not select a file when its directory reveal completes after disposal", async () => {
    mount();
    const release = defer("a/one.txt");
    const opening = view.open("a/one.txt");
    view.dispose();
    release();
    await opening;
    expect(onSelectPath).not.toHaveBeenCalled();
  });

  it("refreshes inline comments while retaining the selected source table and scroll", async () => {
    mount();
    await view.open("b.txt");
    await showing("b.txt");
    const preview = host.querySelector(".trf-preview");
    const source = preview.querySelector(".fsrc");
    const row = source.querySelector("tr");
    preview.scrollTop = 48;
    view.updateComments([{ id: "added", anchor: anchor("b.txt"), body: "A new comment." }]);
    await vi.waitFor(() => expect(host.querySelector('[data-review-inline-comment="added"]')).toBeTruthy());
    expect(preview.querySelector(".fsrc")).toBe(source);
    expect(preview.querySelector('tr[data-new-line="1"]')).toBe(row);
    expect(preview.scrollTop).toBe(48);
  });

  it("retains a playing media element across comment-only updates", async () => {
    mount((path) => ({ ...body(path), mime: "video/mp4" }));
    await view.open("clip.mp4");
    await vi.waitFor(() => expect(host.querySelector("video")?.getAttribute("src")).toMatch(/^blob:/));
    const video = host.querySelector("video");
    video.currentTime = 12;
    const source = video.getAttribute("src");
    view.updateComments([{ id: "note", body: "A timeline-only comment." }]);
    expect(host.querySelector("video")).toBe(video);
    expect(video.currentTime).toBe(12);
    expect(video.getAttribute("src")).toBe(source);
  });

  it("switches a rich preview to source only when a new inline comment requires lines", async () => {
    mount((path) => ({ ...body(path), mime: "text/markdown" }));
    await view.open("README.md");
    await showing("README.md");
    expect(host.querySelector(".trf-preview .plan")).toBeTruthy();
    view.updateComments([{ id: "first", anchor: anchor("README.md"), body: "First comment." }]);
    await vi.waitFor(() => expect(host.querySelector('[data-review-inline-comment="first"]')).toBeTruthy());
    const source = host.querySelector(".trf-preview .fsrc");
    view.updateComments([{ id: "second", anchor: anchor("README.md"), body: "Revised comment." }]);
    expect(host.querySelector(".trf-preview .fsrc")).toBe(source);
    expect(host.querySelector('[data-review-inline-comment="second"]')).toBeTruthy();
  });
});
