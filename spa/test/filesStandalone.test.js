// @vitest-environment jsdom
// The Files tab is one of the two tabs a branch now has, so it must stand on
// its own: a host element, a scope, and an RPC channel — no tab shell, no
// cluster, no surrounding view. This mounts it exactly that way and walks it.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { renderFilesTab } from "../src/views/files.js";

const base64 = (text) => Buffer.from(text, "utf8").toString("base64");

const TREE = {
  "": { path: "", entries: [{ name: "src", kind: "dir" }, { name: "README.md", kind: "file", size: 12 }] },
  src: { path: "src", entries: [{ name: "a.js", kind: "file", size: 20 }] },
};

const FILES = {
  "README.md": { mime: "text/markdown", size: 12, truncated: false, editable: true, encoding: "utf-8", revision: "readme-1", content_b64: base64("# Title\n") },
  "src/a.js": { mime: "text/plain", size: 20, truncated: false, editable: true, encoding: "utf-8", revision: "a-1", content_b64: base64("const a = 1;\n") },
};

function mountFiles({ scope = { project_id: "p1", worktree_id: "w1" }, openAt = null, read = null, write = null, viewingContext = null } = {}) {
  const calls = [];
  const host = document.createElement("div");
  document.body.appendChild(host);
  const files = renderFilesTab(host, {
    scope,
    openAt,
    viewingContext,
    callRpc: async (method, params) => {
      calls.push({ method, params });
      if (method === "fs.tree") return TREE[params.path || ""];
      if (method === "fs.read") return read ? read(params) : FILES[params.path];
      if (method === "fs.write") return write ? write(params) : { ...FILES[params.path], revision: `${FILES[params.path].revision}-saved`, content_b64: params.content_b64 };
      throw new Error(`unexpected ${method}`);
    },
  });
  return { host, files, calls };
}

describe("the Files browser on its own", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("lists the scope root and previews a file, carrying the scope on every call", async () => {
    const { host, files, calls } = mountFiles();
    await vi.waitFor(() => expect(host.querySelector(".ffile")).toBeTruthy());
    expect(host.querySelectorAll(".frow")).toHaveLength(2);

    host.querySelector(".ffile").click();
    await vi.waitFor(() => expect(host.querySelector(".fpbody")).toBeTruthy());
    expect(host.querySelector(".fppath").textContent).toBe("README.md");
    expect(host.querySelector(".fpbody").textContent).toContain("Title");
    // Every fs call is scope-spread: the browser never sends a host path.
    expect(calls.every((call) => call.params.project_id === "p1" && call.params.worktree_id === "w1")).toBe(true);
    files.dispose();
  });

  it("walks into a directory and back out again", async () => {
    const { host, files } = mountFiles();
    await vi.waitFor(() => expect(host.querySelector(".fdir")).toBeTruthy());
    host.querySelector(".fdir").click();
    await vi.waitFor(() => expect(host.querySelector(".fcrumb").textContent).toBe("src"));
    expect(host.querySelector(".ffile").textContent).toContain("a.js");

    host.querySelector(".fup").click();
    await vi.waitFor(() => expect(host.querySelector(".fcrumb").textContent).toBe("/"));
    files.dispose();
  });

  it("opens the file a deep link named, without anything else pointing it there", async () => {
    const { host, files } = mountFiles({ openAt: { path: "src/a.js" } });
    await vi.waitFor(() => expect(host.querySelector(".fppath")).toBeTruthy());
    expect(host.querySelector(".fppath").textContent).toBe("src/a.js");
    expect(host.querySelector(".fcrumb").textContent).toBe("src");
    files.dispose();
  });

  it("says so, and stays usable, when a file cannot be read", async () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const files = renderFilesTab(host, {
      scope: { run_id: "r1" },
      callRpc: async (method) => {
        if (method === "fs.tree") return TREE[""];
        throw new Error("permission denied");
      },
    });
    await vi.waitFor(() => expect(host.querySelector(".ffile")).toBeTruthy());
    host.querySelector(".ffile").click();
    await vi.waitFor(() => expect(host.querySelector(".fpidle").textContent).toContain("permission denied"));
    expect(host.querySelector(".ftree").querySelectorAll(".frow")).toHaveLength(2);
    files.dispose();
  });

  it("edits and saves text with the read revision", async () => {
    const { host, files, calls } = mountFiles();
    await vi.waitFor(() => expect(host.querySelector(".ffile")).toBeTruthy());
    host.querySelector(".ffile").click();
    await vi.waitFor(() => expect(host.querySelector('[data-file-mode="edit"]')).toBeTruthy());
    host.querySelector('[data-file-mode="edit"]').click();
    const editor = host.querySelector(".file-editor");
    editor.value = "# Changed\n";
    editor.dispatchEvent(new Event("input", { bubbles: true }));
    expect(host.querySelector(".file-save").disabled).toBe(false);
    host.querySelector(".file-save").click();
    await vi.waitFor(() => expect(calls.some((call) => call.method === "fs.write")).toBe(true));
    const write = calls.find((call) => call.method === "fs.write");
    expect(write.params).toMatchObject({ project_id: "p1", worktree_id: "w1", path: "README.md", expected_revision: "readme-1" });
    expect(Buffer.from(write.params.content_b64, "base64").toString()).toBe("# Changed\n");
    files.dispose();
  });

  it("keeps the same editor, undo buffer and caret while switching views", async () => {
    const { host, files } = mountFiles();
    await vi.waitFor(() => expect(host.querySelector(".ffile")).toBeTruthy());
    host.querySelector(".ffile").click();
    await vi.waitFor(() => expect(host.querySelector('[data-file-mode="edit"]')).toBeTruthy());
    host.querySelector('[data-file-mode="edit"]').click();
    const editor = host.querySelector(".file-editor");
    editor.value = "# Draft\n";
    editor.setSelectionRange(3, 3);
    editor.dispatchEvent(new Event("input", { bubbles: true }));

    host.querySelector('[data-file-mode="preview"]').click();
    expect(host.querySelector(".file-editor")).toBe(editor);
    expect(host.querySelector(".fpbody").textContent).toContain("Draft");
    host.querySelector('[data-file-mode="edit"]').click();
    expect(host.querySelector(".file-editor")).toBe(editor);
    expect([editor.selectionStart, editor.selectionEnd]).toEqual([3, 3]);
    files.dispose();
  });

  it("keeps a dirty editor when its selected file is retapped or the tree browses elsewhere", async () => {
    const { host, files, calls } = mountFiles();
    await vi.waitFor(() => expect(host.querySelector(".ffile")).toBeTruthy());
    const selectedFile = host.querySelector(".ffile");
    selectedFile.click();
    await vi.waitFor(() => expect(host.querySelector('[data-file-mode="edit"]')).toBeTruthy());
    host.querySelector('[data-file-mode="edit"]').click();
    const editor = host.querySelector(".file-editor");
    editor.value = "draft stays here";
    editor.dispatchEvent(new Event("input", { bubbles: true }));
    const reads = calls.filter(({ method }) => method === "fs.read").length;

    selectedFile.click();
    expect(calls.filter(({ method }) => method === "fs.read")).toHaveLength(reads);
    expect(host.querySelector(".file-editor")).toBe(editor);
    host.querySelector(".fdir").click();
    await vi.waitFor(() => expect(host.querySelector(".fcrumb").textContent).toBe("src"));
    expect(host.querySelector(".file-editor")).toBe(editor);
    expect(editor.value).toBe("draft stays here");
    expect(document.getElementById("confirm-scrim")).toBeNull();
    files.dispose();
  });

  it("saves a retained draft through the new scope after its checkout is adopted", async () => {
    const { host, files, calls } = mountFiles();
    await vi.waitFor(() => expect(host.querySelector(".ffile")).toBeTruthy());
    host.querySelector(".ffile").click();
    await vi.waitFor(() => expect(host.querySelector('[data-file-mode="edit"]')).toBeTruthy());
    host.querySelector('[data-file-mode="edit"]').click();
    const editor = host.querySelector(".file-editor");
    editor.value = "adopted draft";
    editor.dispatchEvent(new Event("input", { bubbles: true }));

    files.retargetScope({ run_id: "run-2" });
    host.querySelector(".file-save").click();
    await vi.waitFor(() => expect(calls.some(({ method }) => method === "fs.write")).toBe(true));
    expect(calls.find(({ method }) => method === "fs.write").params).toMatchObject({ run_id: "run-2", path: "README.md" });
    expect(calls.find(({ method }) => method === "fs.write").params).not.toHaveProperty("worktree_id");
    files.dispose();
  });

  it("ignores a late file read after a newer selection", async () => {
    let finishReadme;
    const read = ({ path }) => path === "README.md"
      ? new Promise((resolve) => { finishReadme = () => resolve(FILES[path]); })
      : FILES[path];
    const { host, files } = mountFiles({ read });
    await vi.waitFor(() => expect(host.querySelector(".ffile")).toBeTruthy());
    host.querySelector(".ffile").click();
    host.querySelector(".fdir").click();
    await vi.waitFor(() => expect(host.querySelector(".fcrumb").textContent).toBe("src"));
    host.querySelector(".ffile").click();
    await vi.waitFor(() => expect(host.querySelector(".fppath").textContent).toBe("src/a.js"));

    finishReadme();
    await Promise.resolve();
    expect(host.querySelector(".fppath").textContent).toBe("src/a.js");
    expect(host.querySelector(".file-reading-layer").textContent).toContain("const a = 1");
    files.dispose();
  });

  it("does not overwrite newer typing when a conflict reload finishes late", async () => {
    let finishReload;
    let reads = 0;
    const read = ({ path }) => ++reads === 1
      ? FILES[path]
      : new Promise((resolve) => { finishReload = () => resolve({ ...FILES[path], revision: "readme-2" }); });
    const write = async () => { throw new Error("revision conflict"); };
    const { host, files } = mountFiles({ read, write });
    await vi.waitFor(() => expect(host.querySelector(".ffile")).toBeTruthy());
    host.querySelector(".ffile").click();
    await vi.waitFor(() => expect(host.querySelector('[data-file-mode="edit"]')).toBeTruthy());
    host.querySelector('[data-file-mode="edit"]').click();
    const editor = host.querySelector(".file-editor");
    editor.value = "conflicting draft";
    editor.dispatchEvent(new Event("input", { bubbles: true }));
    host.querySelector(".file-save").click();
    await vi.waitFor(() => expect(host.querySelector(".file-reload").hidden).toBe(false));
    host.querySelector(".file-reload").click();
    document.querySelector("#confirm-scrim [data-confirm-ok]").click();
    await vi.waitFor(() => expect(finishReload).toEqual(expect.any(Function)));
    editor.value = "newer draft";
    editor.dispatchEvent(new Event("input", { bubbles: true }));

    finishReload();
    await Promise.resolve();
    expect(host.querySelector(".file-editor")).toBe(editor);
    expect(editor.value).toBe("newer draft");
    files.dispose();
  });

  it("keeps newer typing dirty and blocks duplicate saves while an older value is saving", async () => {
    let finishWrite;
    const write = (params) => new Promise((resolve) => {
      finishWrite = () => resolve({ ...FILES[params.path], revision: "readme-2", content_b64: params.content_b64 });
    });
    const { host, files } = mountFiles({ write });
    await vi.waitFor(() => expect(host.querySelector(".ffile")).toBeTruthy());
    host.querySelector(".ffile").click();
    await vi.waitFor(() => expect(host.querySelector('[data-file-mode="edit"]')).toBeTruthy());
    host.querySelector('[data-file-mode="edit"]').click();
    const editor = host.querySelector(".file-editor");
    editor.value = "first";
    editor.dispatchEvent(new Event("input", { bubbles: true }));
    host.querySelector(".file-save").click();
    editor.value = "newer";
    editor.dispatchEvent(new Event("input", { bubbles: true }));
    expect(host.querySelector(".file-save").disabled).toBe(true);
    finishWrite();
    await vi.waitFor(() => expect(host.querySelector(".file-save").disabled).toBe(false));
    expect(host.querySelector(".file-dirty").hidden).toBe(false);
    files.dispose();
  });

  it("keeps its live draft on cancelled route navigation and leaves after confirmation", async () => {
    const viewingContext = { set: vi.fn(), setSelection: vi.fn(), clearSelection: vi.fn(), clear: vi.fn() };
    const { App, go } = await import("../src/app.js");
    const { host, files } = mountFiles({ viewingContext });
    await vi.waitFor(() => expect(host.querySelector(".ffile")).toBeTruthy());
    host.querySelector(".ffile").click();
    await vi.waitFor(() => expect(host.querySelector('[data-file-mode="edit"]')).toBeTruthy());
    host.querySelector('[data-file-mode="edit"]').click();
    const editor = host.querySelector(".file-editor");
    editor.value = "unsaved draft";
    editor.dispatchEvent(new Event("input", { bubbles: true }));
    App.route = { name: "branch", projectId: "p1", branch: "build/edit", tab: "files" };
    App.viewingContext = viewingContext;
    App.routeLeaveGuard = files.canLeave;

    const cancelled = go({ name: "inbox" });
    document.querySelector("#confirm-scrim [data-confirm-cancel]").click();
    expect(await cancelled).toBe(false);
    expect(editor.isConnected).toBe(true);
    expect(editor.value).toBe("unsaved draft");
    expect(App.route.tab).toBe("files");
    expect(viewingContext.clear).not.toHaveBeenCalled();

    const confirmed = go({ name: "inbox" });
    document.querySelector("#confirm-scrim [data-confirm-ok]").click();
    expect(await confirmed).toBe(true);
    expect(App.route).toEqual({ name: "inbox" });
    expect(viewingContext.clear).toHaveBeenCalledTimes(1);
    App.routeLeaveGuard = null;
    files.dispose();
  });

  it("publishes exact visible selections from rendered file content", async () => {
    const viewingContext = { set: vi.fn(), setSelection: vi.fn(), clearSelection: vi.fn(), clear: vi.fn() };
    const { host, files } = mountFiles({ viewingContext });
    await vi.waitFor(() => expect(host.querySelector(".ffile")).toBeTruthy());
    host.querySelector(".ffile").click();
    await vi.waitFor(() => expect(host.querySelector(".plan h1")).toBeTruthy());

    const title = host.querySelector(".plan h1").firstChild;
    const range = document.createRange();
    range.setStart(title, 1);
    range.setEnd(title, 4);
    const selection = document.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    document.dispatchEvent(new Event("selectionchange"));

    expect(viewingContext.set).toHaveBeenCalledWith({ version: 1, items: [{ kind: "file", path: "README.md" }] });
    expect(viewingContext.setSelection).toHaveBeenLastCalledWith([{ kind: "selection", path: "README.md", text: "itl" }]);
    files.dispose();
    expect(viewingContext.clear).toHaveBeenCalled();
  });
});
