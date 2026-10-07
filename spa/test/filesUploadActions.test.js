// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mountFilesUploadActions } from "../src/core/filesUploadActions.js";

const root = { id: "code", label: "Code", scope: { workspace_id: "w", source_id: "code" } };
let actions, host, uploads, callRpc, onCreated;
const row = (path) => host.querySelector(`[data-path="${path}"]`);
const button = (path, action) => row(path).querySelector(`[data-upload-action="${action}"]`);
const key = (element, name) => element.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true }));
const drag = (element, type, transfer = { types: ["Files"], files: [] }) => {
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperty(event, "dataTransfer", { value: transfer });
  element.dispatchEvent(event);
  return event;
};
const mount = (capabilities = { uploads: true, createDirectory: true }) => {
  actions = mountFilesUploadActions(host, { roots: [root], capabilities, uploads, callRpc, onCreated });
};

beforeEach(() => {
  document.body.innerHTML = `<div id="tree"><div class="ftree-list" role="tree"><div class="froot" data-root="code"><div class="frow froot-head" data-root-head="code"><span class="fname">Code</span></div><div class="froot-list"><div class="frow fdir" data-kind="dir" data-path="docs" style="--depth:1"><span class="fname">docs</span></div><div class="frow ffile" data-kind="file" data-path="guide.md"><span class="fname">guide.md</span></div></div></div></div></div>`;
  host = document.querySelector("#tree");
  uploads = { enqueue: vi.fn(), enqueueDrop: vi.fn(async () => {}) };
  callRpc = vi.fn(async (_method, params) => ({ path: `${params.parent}/${params.name}` }));
  onCreated = vi.fn();
});
afterEach(() => actions?.dispose());

describe("cached directory actions", () => {
  it("draws only the actions the cached capabilities offer, including the root", () => {
    mount({ uploads: false, createDirectory: false });
    expect(host.querySelectorAll("[data-upload-action]")).toHaveLength(0);
    actions.setCapabilities({ uploads: true, createDirectory: false });
    expect(host.querySelectorAll('[data-upload-action="upload"]')).toHaveLength(2);
    expect(host.querySelector('[data-upload-action="folder"]')).toBeNull();
    actions.setCapabilities({ uploads: false, createDirectory: true });
    expect(host.querySelectorAll('[data-upload-action="folder"]')).toHaveLength(2);
    expect(host.querySelector('[data-upload-action="upload"]')).toBeNull();
  });
  it("uploads selected files to the clicked directory with flattened selectors", () => {
    mount();
    button("docs", "upload").click();
    const input = host.querySelector('input[type="file"]');
    expect(input.multiple).toBe(true);
    const files = [new File(["a"], "a.txt"), new File(["b"], "b.txt")];
    Object.defineProperty(input, "files", { value: files });
    input.dispatchEvent(new Event("change"));
    expect(uploads.enqueue).toHaveBeenCalledWith(expect.objectContaining({ scope: root.scope, rootId: "code", parent: "docs", files, callRpc }));
  });
  it("creates inline with Enter and cancels with Escape without toggling the row", async () => {
    mount();
    const treeKey = vi.fn();
    host.querySelector(".ftree-list").addEventListener("keydown", treeKey);
    button("docs", "folder").click();
    let input = host.querySelector(".fupload-folder input");
    expect(document.activeElement).toBe(input);
    input.value = "drafts";
    key(input, "Escape");
    expect(host.querySelector(".fupload-folder")).toBeNull();
    button("docs", "folder").click();
    input = host.querySelector(".fupload-folder input");
    input.value = "drafts";
    key(input, "Enter");
    await vi.waitFor(() => expect(onCreated).toHaveBeenCalledWith(root, "docs", "docs/drafts"));
    expect(callRpc).toHaveBeenCalledWith("fs.createDirectory", { workspace_id: "w", source_id: "code", parent: "docs", name: "drafts" });
    expect(treeKey).not.toHaveBeenCalled();
    expect(host.querySelector(".fupload-folder")).toBeNull();
  });
  it("keeps an already_exists message inline and allows a corrected name", async () => {
    mount();
    callRpc.mockRejectedValueOnce(Object.assign(new Error("That folder already exists"), { code: "already_exists" }));
    button("docs", "folder").click();
    const input = host.querySelector(".fupload-folder input");
    input.value = "drafts";
    key(input, "Enter");
    await vi.waitFor(() => expect(host.querySelector('[role="alert"]').textContent).toBe("That folder already exists"));
    expect(input.disabled).toBe(false);
    input.value = "next";
    key(input, "Enter");
    await vi.waitFor(() => expect(onCreated).toHaveBeenCalledWith(root, "docs", "docs/next"));
  });
});

describe("file drops", () => {
  it("marks a directory in place, clears on leave, and enqueues a drop without replacing any rows", () => {
    mount();
    const before = row("docs");
    const html = host.querySelector(".ftree-list").innerHTML;
    drag(before, "dragover");
    expect(before.classList.contains("fupload-drop")).toBe(true);
    expect(before.dataset.uploadHint).toBe("Drop to upload into docs");
    drag(before, "dragleave");
    expect(before.classList.contains("fupload-drop")).toBe(false);
    expect(host.querySelector(".ftree-list").innerHTML).toBe(html);
    const dataTransfer = { types: ["Files"], files: [new File(["a"], "a.txt")] };
    drag(before, "drop", dataTransfer);
    expect(uploads.enqueueDrop).toHaveBeenCalledWith(expect.objectContaining({ scope: root.scope, parent: "docs", dataTransfer, rootId: "code" }));
    expect(row("docs")).toBe(before);
    expect(host.querySelector(".fupload-drop")).toBeNull();
  });
  it("targets a file's parent and empty tree space at the root", () => {
    mount();
    drag(row("guide.md"), "drop");
    drag(host, "drop");
    expect(uploads.enqueueDrop.mock.calls.map(([params]) => params.parent)).toEqual(["", ""]);
  });
  it("ignores text drags and explains file drops on an older bridge", () => {
    mount({ uploads: false, createDirectory: false });
    expect(drag(row("docs"), "dragover", { types: ["text/plain"] }).defaultPrevented).toBe(false);
    drag(row("docs"), "dragover");
    expect(host.querySelector(".fupload-drop")).toBeNull();
    drag(row("docs"), "drop");
    expect(document.querySelector(".notice-summary").textContent).toBe("Update the bridge to upload files");
    expect(uploads.enqueueDrop).not.toHaveBeenCalled();
  });
});
