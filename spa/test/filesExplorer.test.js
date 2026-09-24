// @vitest-environment jsdom
// The Files tab as an IDE explorer (#151): the open file's row is highlighted
// on every paint, a fine pointer selects on click and opens on double-click
// while a touch opens on tap, opened files are closable tabs over the preview,
// and the expanded directories and open tabs come back on the next mount.

import { describe, it, expect, afterEach, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { scopeFor } = await import("../src/core/cacheScope.js");
const { wipeCache, writeCached } = await import("../src/core/localCache.js");
const { renderFilesTab } = await import("../src/views/files.js");

const b64 = (text) => Buffer.from(text, "utf8").toString("base64");

const TREE = {
  "": [{ name: "src", kind: "dir" }, { name: "README.md", kind: "file", size: 8 }, { name: "notes.txt", kind: "file", size: 6 }],
  src: [{ name: "core", kind: "dir" }, { name: "a.js", kind: "file", size: 13 }],
  "src/core": [{ name: "b.js", kind: "file", size: 13 }],
};

const BODIES = {
  "README.md": "# Title\n",
  "notes.txt": "notes\n",
  "src/a.js": "const a = 1;\n",
  "src/core/b.js": "const b = 2;\n",
};

const answer = vi.fn(async (method, params) => {
  if (method === "fs.tree") return { path: params.path, entries: TREE[params.path] || [] };
  if (method === "fs.read") {
    const text = BODIES[params.path];
    return { path: params.path, mime: "text/plain", size: text.length, truncated: false, editable: true, encoding: "utf-8", revision: `${params.path}@1`, content_b64: b64(text) };
  }
  throw new Error(`unexpected ${method}`);
});

/** Stand the page on one kind of pointer: `fine` answers (pointer: fine). */
const pointer = (kind) => {
  window.matchMedia = (query) => ({ matches: query === "(pointer: fine)" && kind === "fine", media: query, addEventListener() {}, removeEventListener() {} });
};

const mounted = [];

const mountFiles = ({ openAt = null, onFileOpen = null } = {}) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const files = renderFilesTab(host, { scope: { run_id: "run-1" }, callRpc: answer, cacheScope: scopeFor("dev-1"), openAt, onFileOpen });
  mounted.push(files);
  return { host, files };
};

// A remount replaces the old tab's DOM, as the surfaces do.
const unmount = (host) => {
  mounted.splice(0).forEach((files) => files.dispose());
  host.remove();
};

const rowFor = (host, path) => [...host.querySelectorAll(".frow[data-path]")].find((row) => row.dataset.path === path);
const drawn = (host) => [...host.querySelectorAll(".frow[data-path]")].map((row) => row.dataset.path);
const tabNames = (host) => [...host.querySelectorAll(".ftab-name")].map((tab) => tab.dataset.tabPath);
const activeTab = (host) => host.querySelector('.ftab-name[aria-selected="true"]')?.dataset.tabPath;
const shownPath = (host) => host.querySelector(".fppath")?.textContent;

const click = (element, detail = 1) => element.dispatchEvent(new MouseEvent("click", { bubbles: true, detail }));
const doubleClick = (element) => {
  click(element, 1);
  click(element, 2);
  element.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, detail: 2 }));
};
const key = (element, name) => element.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true }));

const settle = async () => {
  for (let i = 0; i < 30; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};

const openByTap = async (host, path) => {
  await vi.waitFor(() => expect(rowFor(host, path)).toBeTruthy());
  click(rowFor(host, path));
  await vi.waitFor(() => expect(shownPath(host)).toBe(path));
};

const typeInto = (host, text) => {
  host.querySelector('[data-file-mode="edit"]').click();
  const editor = host.querySelector(".file-editor");
  editor.value = text;
  editor.dispatchEvent(new Event("input", { bubbles: true }));
  return editor;
};

beforeEach(async () => {
  document.body.innerHTML = "";
  answer.mockClear();
  pointer("coarse");
  await wipeCache();
});

afterEach(() => {
  mounted.splice(0).forEach((files) => files.dispose());
  delete window.matchMedia;
});

describe("the open file's row", () => {
  it("stays highlighted when a push repaints the listing it is in", async () => {
    const { host } = mountFiles();
    await openByTap(host, "README.md");
    expect(rowFor(host, "README.md").classList.contains("sel")).toBe(true);

    await writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "" }, { path: "", entries: [...TREE[""], { name: "zz.txt", kind: "file", size: 1 }] });
    await vi.waitFor(() => expect(rowFor(host, "zz.txt")).toBeTruthy());
    expect(rowFor(host, "README.md").classList.contains("sel")).toBe(true);
    expect(rowFor(host, "README.md").getAttribute("aria-current")).toBe("true");
    expect(host.querySelectorAll(".frow.sel")).toHaveLength(1);
  });

  it("is highlighted after a route opens it, and again after its directory collapses and expands", async () => {
    const { host } = mountFiles({ openAt: { path: "src/core/b.js" } });
    await vi.waitFor(() => expect(shownPath(host)).toBe("src/core/b.js"));
    await vi.waitFor(() => expect(rowFor(host, "src/core/b.js")?.classList.contains("sel")).toBe(true));

    click(rowFor(host, "src/core"));
    await vi.waitFor(() => expect(rowFor(host, "src/core/b.js")).toBeUndefined());
    expect(host.querySelector(".frow.sel")).toBeNull();

    click(rowFor(host, "src/core"));
    await vi.waitFor(() => expect(rowFor(host, "src/core/b.js")?.classList.contains("sel")).toBe(true));
  });
});

describe("on a fine pointer", () => {
  beforeEach(() => pointer("fine"));

  it("selects a file on click without opening it, and opens it on double-click", async () => {
    const { host } = mountFiles();
    await vi.waitFor(() => expect(rowFor(host, "README.md")).toBeTruthy());
    click(rowFor(host, "README.md"));
    await settle();
    const row = rowFor(host, "README.md");
    expect(row.classList.contains("cursor")).toBe(true);
    expect(row.getAttribute("aria-selected")).toBe("true");
    expect(row.classList.contains("sel")).toBe(false);
    expect(shownPath(host)).toBeUndefined();
    expect(answer.mock.calls.some(([method]) => method === "fs.read")).toBe(false);

    doubleClick(rowFor(host, "README.md"));
    await vi.waitFor(() => expect(shownPath(host)).toBe("README.md"));
    expect(rowFor(host, "README.md").classList.contains("sel")).toBe(true);
  });

  it("keeps the open file highlighted while the selection is elsewhere, one selected row at a time", async () => {
    const { host } = mountFiles();
    await vi.waitFor(() => expect(rowFor(host, "README.md")).toBeTruthy());
    doubleClick(rowFor(host, "README.md"));
    await vi.waitFor(() => expect(shownPath(host)).toBe("README.md"));
    click(rowFor(host, "notes.txt"));

    expect(rowFor(host, "README.md").classList.contains("sel")).toBe(true);
    expect(rowFor(host, "README.md").classList.contains("cursor")).toBe(false);
    expect(rowFor(host, "notes.txt").classList.contains("cursor")).toBe(true);
    expect(host.querySelectorAll(".frow.cursor")).toHaveLength(1);
    expect(host.querySelectorAll('.frow[aria-selected="true"]')).toHaveLength(1);
  });

  it("toggles a directory once on a double-click, not twice", async () => {
    const { host } = mountFiles();
    await vi.waitFor(() => expect(rowFor(host, "src")).toBeTruthy());
    doubleClick(rowFor(host, "src"));
    await vi.waitFor(() => expect(rowFor(host, "src/a.js")).toBeTruthy());
    await settle();
    expect(rowFor(host, "src").getAttribute("aria-expanded")).toBe("true");
  });

  it("moves the selection with the arrows, expands and collapses with Right and Left, opens with Enter", async () => {
    const { host } = mountFiles();
    await vi.waitFor(() => expect(rowFor(host, "src")).toBeTruthy());
    click(rowFor(host, "notes.txt"));
    const list = host.querySelector(".ftree-list");

    key(list, "ArrowUp");
    key(list, "ArrowUp");
    expect(host.querySelector(".frow.cursor").dataset.path).toBe("src");
    expect(document.activeElement).toBe(rowFor(host, "src"));

    key(list, "ArrowRight");
    await vi.waitFor(() => expect(drawn(host)).toEqual(["src", "src/core", "src/a.js", "README.md", "notes.txt"]));
    key(list, "ArrowDown");
    key(list, "ArrowDown");
    expect(host.querySelector(".frow.cursor").dataset.path).toBe("src/a.js");

    key(list, "Enter");
    await vi.waitFor(() => expect(shownPath(host)).toBe("src/a.js"));

    key(list, "ArrowLeft");
    expect(host.querySelector(".frow.cursor").dataset.path).toBe("src");
    key(list, "ArrowLeft");
    await vi.waitFor(() => expect(drawn(host)).toEqual(["src", "README.md", "notes.txt"]));
    expect(document.activeElement).toBe(rowFor(host, "src"));
  });
});

describe("on a touch screen", () => {
  it("opens a file on a single tap and puts the drawer away", async () => {
    const { host } = mountFiles();
    await vi.waitFor(() => expect(rowFor(host, "README.md")).toBeTruthy());
    const split = host.querySelector(".files");
    split.querySelector("[data-pane-handle]").click();
    expect(split.classList.contains("drawer-open")).toBe(true);
    click(rowFor(host, "README.md"));
    expect(split.classList.contains("drawer-open")).toBe(false);
    await vi.waitFor(() => expect(shownPath(host)).toBe("README.md"));
  });
});

describe("the open files' tabs", () => {
  it("opens each file as a tab, activates an open one instead of duplicating it, and switches on a click", async () => {
    const { host } = mountFiles();
    await openByTap(host, "README.md");
    await openByTap(host, "notes.txt");
    expect(tabNames(host)).toEqual(["README.md", "notes.txt"]);
    expect(activeTab(host)).toBe("notes.txt");

    await openByTap(host, "README.md");
    expect(tabNames(host)).toEqual(["README.md", "notes.txt"]);
    expect(activeTab(host)).toBe("README.md");

    host.querySelector('[data-tab-path="notes.txt"]').click();
    await vi.waitFor(() => expect(shownPath(host)).toBe("notes.txt"));
    expect(activeTab(host)).toBe("notes.txt");
    expect(rowFor(host, "notes.txt").classList.contains("sel")).toBe(true);
    expect(rowFor(host, "README.md").classList.contains("sel")).toBe(false);
  });

  it("closing the active tab shows its neighbour, and closing the last shows the empty preview", async () => {
    const opened = [];
    const { host } = mountFiles({ onFileOpen: (path) => opened.push(path) });
    await openByTap(host, "README.md");
    await openByTap(host, "notes.txt");

    host.querySelector('[data-tab-close="notes.txt"]').click();
    await vi.waitFor(() => expect(shownPath(host)).toBe("README.md"));
    expect(tabNames(host)).toEqual(["README.md"]);

    host.querySelector('[data-tab-close="README.md"]').click();
    await vi.waitFor(() => expect(host.querySelector(".fpidle-msg")?.textContent).toBe("No file open"));
    expect(host.querySelector(".ftabs").hidden).toBe(true);
    expect(host.querySelector(".frow.sel")).toBeNull();
    expect(opened.at(-1)).toBeNull();
  });

  it("keeps a tab's unsaved edits while another tab is shown, and marks the tab", async () => {
    const { host } = mountFiles();
    await openByTap(host, "README.md");
    typeInto(host, "draft in README");
    await vi.waitFor(() => expect(host.querySelector('.ftab.dirty [data-tab-path="README.md"]')).toBeTruthy());

    await openByTap(host, "notes.txt");
    expect(host.querySelector('.ftab.dirty [data-tab-path="README.md"]')).toBeTruthy();
    expect(host.querySelector(".file-editor-layer").hidden).toBe(true);

    host.querySelector('[data-tab-path="README.md"]').click();
    await vi.waitFor(() => expect(shownPath(host)).toBe("README.md"));
    expect(host.querySelector(".file-editor").value).toBe("draft in README");
    expect(document.getElementById("confirm-scrim")).toBeNull();
  });

  it("asks before closing a tab with unsaved edits, and keeps it when the answer is no", async () => {
    const { host, files } = mountFiles();
    await openByTap(host, "README.md");
    typeInto(host, "unsaved");
    await vi.waitFor(() => expect(host.querySelector(".ftab.dirty")).toBeTruthy());

    host.querySelector('[data-tab-close="README.md"]').click();
    await vi.waitFor(() => expect(document.querySelector("#confirm-scrim [data-confirm-cancel]")).toBeTruthy());
    document.querySelector("#confirm-scrim [data-confirm-cancel]").click();
    await settle();
    expect(tabNames(host)).toEqual(["README.md"]);
    expect(host.querySelector(".file-editor").value).toBe("unsaved");

    host.querySelector('[data-tab-close="README.md"]').click();
    await vi.waitFor(() => expect(document.querySelector("#confirm-scrim [data-confirm-ok]")).toBeTruthy());
    document.querySelector("#confirm-scrim [data-confirm-ok]").click();
    await vi.waitFor(() => expect(tabNames(host)).toEqual([]));
    expect(files.hasUnsavedChanges()).toBe(false);
  });

  it("asks about the shown file's edits alone when it is reloaded", async () => {
    const { host } = mountFiles();
    await openByTap(host, "README.md");
    typeInto(host, "background draft");
    await openByTap(host, "notes.txt");
    typeInto(host, "shown draft");
    await writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "file", sub: "notes.txt" },
      { file: { path: "notes.txt", mime: "text/plain", size: 7, truncated: false, editable: true, encoding: "utf-8", revision: "notes.txt@2", content_b64: b64("moved\n") }, openedAt: Date.now() },
    );
    await vi.waitFor(() => expect(host.querySelector(".file-reload").hidden).toBe(false));
    host.querySelector(".file-reload").click();
    await vi.waitFor(() => expect(document.querySelector("#confirm-scrim")).toBeTruthy());
    expect(document.querySelector("#confirm-scrim").textContent).toContain("notes.txt");
    expect(document.querySelector("#confirm-scrim").textContent).not.toContain("README.md");
    document.querySelector("#confirm-scrim [data-confirm-cancel]").click();
  });

  it("counts a background tab's unsaved edits when the view is left", async () => {
    const { host, files } = mountFiles();
    await openByTap(host, "README.md");
    typeInto(host, "held in the background");
    await openByTap(host, "notes.txt");
    expect(files.hasUnsavedChanges()).toBe(true);
  });
});

describe("the next mount of the same checkout", () => {
  it("comes back with the same directories expanded and the same tabs open, the active one shown", async () => {
    const first = mountFiles();
    await vi.waitFor(() => expect(rowFor(first.host, "src")).toBeTruthy());
    click(rowFor(first.host, "src"));
    await vi.waitFor(() => expect(rowFor(first.host, "src/core")).toBeTruthy());
    click(rowFor(first.host, "src/core"));
    await openByTap(first.host, "src/core/b.js");
    await openByTap(first.host, "README.md");
    await openByTap(first.host, "src/a.js");
    first.host.querySelector('[data-tab-path="README.md"]').click();
    await vi.waitFor(() => expect(shownPath(first.host)).toBe("README.md"));
    unmount(first.host);

    const second = mountFiles();
    await vi.waitFor(() => expect(drawn(second.host)).toEqual(["src", "src/core", "src/core/b.js", "src/a.js", "README.md", "notes.txt"]));
    await vi.waitFor(() => expect(shownPath(second.host)).toBe("README.md"));
    expect(tabNames(second.host)).toEqual(["src/core/b.js", "README.md", "src/a.js"]);
    expect(activeTab(second.host)).toBe("README.md");
    expect(rowFor(second.host, "README.md").classList.contains("sel")).toBe(true);
  });

  it("opens the route's file on top of the remembered tabs", async () => {
    const first = mountFiles();
    await openByTap(first.host, "README.md");
    unmount(first.host);

    const second = mountFiles({ openAt: { path: "notes.txt" } });
    await vi.waitFor(() => expect(shownPath(second.host)).toBe("notes.txt"));
    expect(tabNames(second.host)).toEqual(["README.md", "notes.txt"]);
    expect(answer.mock.calls.filter(([method, params]) => method === "fs.read" && params.path === "README.md")).toHaveLength(1);
  });
});
