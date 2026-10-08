// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mountFilesUploadActions } from "../src/core/filesUploadActions.js";
import { mountFileTree } from "../src/core/fileTree.js";

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
afterEach(() => {
  actions?.dispose();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("cached directory actions", () => {
  it("renders recognizable SVG icons with accessible labels", () => {
    mount();
    for (const [action, label] of [["upload", "Upload files"], ["folder", "New folder"]]) {
      const control = button("docs", action);
      expect(control.getAttribute("aria-label")).toBe(label);
      expect(control.querySelector("svg")).not.toBeNull();
      expect(control.textContent.trim()).toBe("");
    }
  });
  it("keeps the decorated SVG buttons and their focus across unchanged capabilities", () => {
    mount();
    const control = button("docs", "upload");
    control.focus();
    actions.setCapabilities({ uploads: true, createDirectory: true });
    expect(button("docs", "upload")).toBe(control);
    expect(document.activeElement).toBe(control);
  });
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
  it("reports successful folder creation even if the view was disposed while awaiting it", async () => {
    mount();
    let resolve;
    callRpc.mockImplementation(() => new Promise((done) => { resolve = done; }));
    button("docs", "folder").click();
    const input = host.querySelector(".fupload-folder input"); input.value = "drafts";
    key(input, "Enter");
    actions.dispose();
    resolve({ path: "docs/drafts" });
    await vi.waitFor(() => expect(onCreated).toHaveBeenCalledWith(root, "docs", "docs/drafts"));
  });
});

describe("touch directory actions", () => {
  const pointer = (element, type, props = {}) => {
    const event = new Event(type, { bubbles: true, cancelable: true });
    Object.assign(event, { pointerId: 1, pointerType: "touch", button: 0, isPrimary: true, clientX: 20, clientY: 20 }, props);
    element.dispatchEvent(event);
    return event;
  };
  const click = (element) => element.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 }));
  const revealed = () => [...host.querySelectorAll(".fupload-revealed")];
  beforeEach(() => {
    vi.useFakeTimers();
    // A touchscreen laptop can have a fine primary pointer and a coarse
    // secondary one; eligibility must ask about every attached pointer.
    vi.stubGlobal("matchMedia", vi.fn((query) => ({ matches: query === "(any-pointer: coarse)" })));
    row("docs").insertAdjacentHTML("afterend", '<div class="frow fdir" data-kind="dir" data-path="assets"><span class="fname">assets</span></div>');
    mount();
  });
  it("reveals exactly the held row after 600 ms and prevents its native context menu", () => {
    pointer(row("docs"), "pointerdown");
    vi.advanceTimersByTime(600);
    expect(revealed()).toEqual([row("docs")]);
    const menu = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    row("docs").dispatchEvent(menu);
    expect(menu.defaultPrevented).toBe(true);
    pointer(row("docs"), "pointerup");
    const navigation = vi.fn();
    host.addEventListener("click", navigation);
    click(row("docs"));
    expect(navigation).not.toHaveBeenCalled();
    expect(revealed()).toEqual([row("docs")]);
  });
  it("keeps a 200 ms tap available for directory navigation without revealing actions", async () => {
    const tree = mountFileTree(host.querySelector(".ftree-list"), {
      listingAddress: () => null, readsForItself: () => false, finePointer: () => false,
      listDirectory: async (path) => ({ entries: path ? [] : [{ name: "docs", kind: "dir" }] }), onOpen: vi.fn(),
    });
    await vi.waitFor(() => expect(row("docs")?.getAttribute("aria-expanded")).toBe("false"));
    pointer(row("docs"), "pointerdown");
    vi.advanceTimersByTime(200);
    pointer(row("docs"), "pointerup");
    click(row("docs"));
    vi.advanceTimersByTime(400);
    expect(revealed()).toEqual([]);
    expect(row("docs").getAttribute("aria-expanded")).toBe("true");
    tree.dispose();
  });
  it("restores ordinary context menus after release", () => {
    pointer(row("docs"), "pointerdown");
    vi.advanceTimersByTime(600);
    pointer(row("docs"), "pointerup");
    const delayedMenu = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    Object.assign(delayedMenu, { pointerType: "touch" });
    row("docs").dispatchEvent(delayedMenu);
    expect(delayedMenu.defaultPrevented).toBe(true);
    click(row("docs"));
    const menu = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    Object.assign(menu, { pointerType: "touch" });
    row("docs").dispatchEvent(menu);
    expect(menu.defaultPrevented).toBe(false);
  });
  it("cancels when a pending row disappears or the tree scrolls", () => {
    pointer(row("docs"), "pointerdown");
    row("docs").remove();
    vi.advanceTimersByTime(600);
    expect(revealed()).toEqual([]);
    pointer(row("assets"), "pointerdown");
    host.dispatchEvent(new Event("scroll"));
    vi.advanceTimersByTime(600);
    expect(revealed()).toEqual([]);
  });
  it("cancels a reveal after moving 12 px", () => {
    pointer(row("docs"), "pointerdown");
    vi.advanceTimersByTime(200);
    pointer(document.body, "pointermove", { clientX: 32 });
    vi.advanceTimersByTime(400);
    expect(revealed()).toEqual([]);
  });
  it.each(["pointerup", "pointercancel"])("cancels a pending reveal on %s outside the tree", (type) => {
    pointer(row("docs"), "pointerdown");
    vi.advanceTimersByTime(200);
    pointer(document.body, type);
    vi.advanceTimersByTime(400);
    expect(revealed()).toEqual([]);
  });
  it("replaces the revealed row and dismisses on a tap elsewhere", () => {
    pointer(row("docs"), "pointerdown");
    vi.advanceTimersByTime(600);
    pointer(row("docs"), "pointerup");
    pointer(row("assets"), "pointerdown");
    expect(revealed()).toEqual([]);
    vi.advanceTimersByTime(600);
    expect(revealed()).toEqual([row("assets")]);
    pointer(row("assets"), "pointerup");
    pointer(document.body, "pointerdown");
    expect(revealed()).toEqual([]);
  });
  it("lets the next touch activate an action after a long press", () => {
    pointer(row("docs"), "pointerdown");
    vi.advanceTimersByTime(600);
    pointer(row("docs"), "pointerup");
    pointer(button("docs", "folder"), "pointerdown");
    pointer(button("docs", "folder"), "pointerup");
    click(button("docs", "folder"));
    expect(host.querySelector('[aria-label="New folder name"]')).not.toBeNull();
  });
  it("does not intercept file context menus or fine-pointer holds", () => {
    const menu = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    row("guide.md").dispatchEvent(menu);
    expect(menu.defaultPrevented).toBe(false);
    window.matchMedia.mockReturnValue({ matches: false });
    pointer(row("docs"), "pointerdown", { pointerType: "mouse" });
    vi.advanceTimersByTime(600);
    expect(revealed()).toEqual([]);
  });
  it("cleans up revealed actions on capability changes and disposal", () => {
    pointer(row("docs"), "pointerdown");
    vi.advanceTimersByTime(600);
    actions.setCapabilities({ uploads: false, createDirectory: false });
    expect(revealed()).toEqual([]);
    actions.setCapabilities({ uploads: true, createDirectory: true });
    pointer(row("docs"), "pointerdown");
    actions.dispose();
    vi.advanceTimersByTime(600);
    expect(revealed()).toEqual([]);
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

it("ignores multi-root empty space and targets explicit second-root files", () => {
  const second = { id: "docs", label: "Docs", scope: { project_id: "p", source_id: "docs" } };
  host.querySelector(".ftree-list").insertAdjacentHTML("beforeend", '<div class="froot" data-root="docs"><div class="frow froot-head" data-root-head="docs">Docs</div><div class="frow" data-kind="file" data-path="readme.md">readme</div></div>');
  actions = mountFilesUploadActions(host, { roots: [root, second], capabilities: { uploads: true }, uploads, callRpc, onCreated });
  drag(host, "dragover");
  expect(host.querySelector(".fupload-drop")).toBeNull();
  drag(host, "drop");
  expect(uploads.enqueueDrop).not.toHaveBeenCalled();
  const explicit = host.querySelector('[data-root="docs"] [data-kind="file"]');
  drag(explicit, "dragover");
  expect(host.querySelector('[data-root-head="docs"]').classList.contains("fupload-drop")).toBe(true);
  drag(explicit, "drop");
  expect(uploads.enqueueDrop).toHaveBeenCalledWith(expect.objectContaining({ rootId: "docs", scope: second.scope, parent: "" }));
});
it("allows native file drop dispatch on an older bridge", () => {
  mount({ uploads: false });
  const transfer = { types: ["Files"], dropEffect: "none" };
  drag(row("docs"), "dragover", transfer);
  expect(transfer.dropEffect).toBe("copy");
});
it.each(["", ".", "..", "bad/name", "bad\\name", "bad\0name"])("rejects invalid new-folder name %j locally", (name) => {
  mount();
  button("docs", "folder").click();
  const input = host.querySelector(".fupload-folder input");
  input.value = name;
  key(input, "Enter");
  expect(callRpc).not.toHaveBeenCalled();
  expect(host.querySelector('[role="alert"]').textContent).toContain("folder name");
});
