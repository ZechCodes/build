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
const { readCached, wipeCache, writeCached } = await import("../src/core/localCache.js");
const { cacheFileBody } = await import("../src/core/cacheLifetime.js");
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
  if (method === "fs.write") return writeAnswer(params);
  throw new Error(`unexpected ${method}`);
});

// fs.write answers whatever the test holds out; by default it never answers.
let writeAnswer = () => new Promise(() => {});

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
  writeAnswer = () => new Promise(() => {});
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

  it("finishes a save that lands while another tab is shown, and saves that file's next edits", async () => {
    let land;
    writeAnswer = (params) => new Promise((resolve) => {
      land = () => resolve({ path: params.path, size: 5, revision: `${params.path}@2`, content_b64: params.content_b64 });
    });
    const { host, files } = mountFiles();
    await openByTap(host, "README.md");
    typeInto(host, "one\n");
    host.querySelector(".file-save").click();
    await vi.waitFor(() => expect(land).toBeTruthy());

    await openByTap(host, "notes.txt");
    land();
    await settle();
    expect(files.hasUnsavedChanges()).toBe(false);

    host.querySelector('[data-tab-path="README.md"]').click();
    await vi.waitFor(() => expect(shownPath(host)).toBe("README.md"));
    await settle();
    expect(host.querySelector(".file-editor").value).toBe("one\n");
    expect(host.querySelector(".file-save-status").textContent).toBe("");
    typeInto(host, "two\n");
    expect(host.querySelector(".file-save").disabled).toBe(false);
    host.querySelector(".file-save").click();
    await vi.waitFor(() => expect(answer.mock.calls.filter(([method]) => method === "fs.write")).toHaveLength(2));
    expect(answer.mock.calls.at(-1)[1]).toMatchObject({ path: "README.md", expected_revision: "README.md@2" });
  });

  it("brings a dirty tab back with its disk-change warning, whether the change came before or while it was away", async () => {
    const moved = (path, text) => writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "file", sub: path },
      { file: { path, mime: "text/plain", size: text.length, truncated: false, editable: true, encoding: "utf-8", revision: `${path}@${text.length}`, content_b64: b64(text) }, openedAt: Date.now() },
    );
    const warned = (host) => host.querySelector(".file-save-status").textContent === "File changed on disk" && !host.querySelector(".file-reload").hidden;
    const { host } = mountFiles();
    await openByTap(host, "README.md");
    await openByTap(host, "notes.txt");
    host.querySelector('[data-tab-path="README.md"]').click();
    await vi.waitFor(() => expect(shownPath(host)).toBe("README.md"));
    typeInto(host, "draft\n");
    await moved("README.md", "moved\n");
    await vi.waitFor(() => expect(warned(host)).toBe(true));

    host.querySelector('[data-tab-path="notes.txt"]').click();
    await vi.waitFor(() => expect(shownPath(host)).toBe("notes.txt"));
    host.querySelector('[data-tab-path="README.md"]').click();
    await vi.waitFor(() => expect(shownPath(host)).toBe("README.md"));
    await vi.waitFor(() => expect(warned(host)).toBe(true));
    expect(host.querySelector(".file-editor").value).toBe("draft\n");

    host.querySelector('[data-tab-path="notes.txt"]').click();
    await vi.waitFor(() => expect(shownPath(host)).toBe("notes.txt"));
    typeInto(host, "notes draft\n");
    host.querySelector('[data-tab-path="README.md"]').click();
    await vi.waitFor(() => expect(shownPath(host)).toBe("README.md"));
    await vi.waitFor(() => expect(warned(host)).toBe(true));
    await moved("notes.txt", "moved on disk\n");
    host.querySelector('[data-tab-path="notes.txt"]').click();
    await vi.waitFor(() => expect(shownPath(host)).toBe("notes.txt"));
    await vi.waitFor(() => expect(warned(host)).toBe(true));
    expect(host.querySelector(".file-editor").value).toBe("notes draft\n");
  });

  it("counts a background tab's unsaved edits when the view is left", async () => {
    const { host, files } = mountFiles();
    await openByTap(host, "README.md");
    typeInto(host, "held in the background");
    await openByTap(host, "notes.txt");
    expect(files.hasUnsavedChanges()).toBe(true);
  });
});

// Every state a tab's draft can be in, crossed with every event that moves it,
// through the whole view: the edit buffer, the tab's dirty dot, Save, the
// status line and Reload, and the revision the next save is sent against
// (“base”; null while a save is still out). README.md is read at
// README.md@1; an acknowledged save answers README.md@2; another writer's
// change lands in the cache as README.md@ext.
describe("a tab's draft, state by event", () => {
  const README = "# Title\n";
  const DISK = "File changed on disk";
  const CONFLICT = "revision conflict: README.md changed";
  const readmeAddress = { deviceId: "dev-1", entityId: "run-1", kind: "file", sub: "README.md" };
  let writes;

  beforeEach(() => {
    writes = [];
    writeAnswer = (params) => new Promise((resolve, reject) => {
      const write = { params, open: true };
      write.land = () => {
        write.open = false;
        resolve({ path: params.path, mime: "text/plain", size: 9, truncated: false, editable: true, encoding: "utf-8", revision: "README.md@2", content_b64: params.content_b64 });
      };
      write.refuse = () => {
        write.open = false;
        reject(new Error(CONFLICT));
      };
      writes.push(write);
    });
  });

  const pushExternal = async () => {
    await writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "file", sub: "README.md" },
      { file: { path: "README.md", mime: "text/plain", size: 6, truncated: false, editable: true, encoding: "utf-8", revision: "README.md@ext", content_b64: b64("moved\n") }, openedAt: Date.now() },
    );
    await settle();
  };
  const refreshReadme = async (competing = false) => {
    const before = await readCached(readmeAddress);
    // A second opener refreshes access time without changing the baseline.
    // A competing revision can even have the same bytes; its revision wins.
    const file = { ...before.value.file, ...(competing ? { revision: "README.md@3" } : {}) };
    await settle();
    await cacheFileBody({ ...readmeAddress, path: "README.md", file });
    const after = await readCached(readmeAddress);
    expect(after.at).toBeGreaterThan(before.at);
    expect(after.value.file).toEqual(file);
    await settle();
  };
  const show = async (host, path) => {
    host.querySelector(`[data-tab-path="${path}"]`).click();
    await vi.waitFor(() => expect(shownPath(host)).toBe(path));
    await settle();
  };
  const save = async (host) => {
    const sent = writes.length;
    host.querySelector(".file-save").click();
    await vi.waitFor(() => expect(writes).toHaveLength(sent + 1));
  };
  const saveOut = async (host) => {
    if (!writes.some((write) => write.open)) await save(host);
  };
  const land = async () => {
    writes.find((write) => write.open).land();
    await settle();
  };
  const refuse = async () => {
    writes.find((write) => write.open).refuse();
    await settle();
  };
  const dot = (host) => Boolean(host.querySelector('.ftab.dirty [data-tab-path="README.md"]'));
  const away = async (host, meanwhile) => {
    await show(host, "notes.txt");
    await meanwhile();
    const dotAway = dot(host);
    await show(host, "README.md");
    return dotAway;
  };
  const confirmShown = async () => {
    await settle();
    const ok = document.querySelector("#confirm-scrim [data-confirm-ok]");
    ok?.click();
    await settle();
    return Boolean(ok);
  };
  const remountPendingSave = async (host) => {
    const before = await readCached(readmeAddress);
    const reads = answer.mock.calls.filter(([method]) => method === "fs.read").length;
    await settle();
    unmount(host);
    const next = mountFiles();
    await vi.waitFor(() => expect(shownPath(next.host)).toBe("README.md"));
    await settle();
    const refreshed = await readCached(readmeAddress);
    expect(refreshed.at).toBeGreaterThan(before.at);
    expect(refreshed.value.file).toEqual(before.value.file);
    expect(answer.mock.calls.filter(([method]) => method === "fs.read")).toHaveLength(reads);
    return next.host;
  };

  const STATES = {
    clean: async () => {},
    dirty: async (host) => typeInto(host, "draft\n"),
    saving: async (host) => {
      typeInto(host, "draft\n");
      await save(host);
    },
    // Typed back to the very text the file had before the save went out.
    "saving-edited": async (host) => {
      typeInto(host, "draft\n");
      await save(host);
      typeInto(host, README);
    },
    "changed-on-disk": async (host) => {
      typeInto(host, "draft\n");
      await pushExternal();
      await vi.waitFor(() => expect(host.querySelector(".file-save-status").textContent).toBe(DISK));
    },
  };

  // Each event answers what it saw on the way: whether the tab's dot was on
  // while another tab was shown, and whether it asked before discarding.
  const EVENTS = {
    "save ok": async (host) => {
      await saveOut(host);
      await land();
    },
    "save refused": async (host) => {
      await saveOut(host);
      await refuse();
    },
    push: pushExternal,
    "push, then save ok": async () => {
      await pushExternal();
      await land();
    },
    "switch away and back": async (host) => ({ dotAway: await away(host, async () => {}) }),
    "save ok while away": async (host) => {
      await saveOut(host);
      return { dotAway: await away(host, land) };
    },
    "push while away": async (host) => ({ dotAway: await away(host, pushExternal) }),
    "push, then save ok, while away": async (host) => {
      await saveOut(host);
      return { dotAway: await away(host, async () => { await pushExternal(); await land(); }) };
    },
    "same-body refresh, then save ok, while away": async (host) => ({
      dotAway: await away(host, async () => { await refreshReadme(); await land(); }),
    }),
    "r3 with baseline bytes, then save ok, while away": async (host) => ({
      dotAway: await away(host, async () => { await refreshReadme(true); await land(); }),
    }),
    "remount, then save ok": async (host) => {
      const nextHost = await remountPendingSave(host);
      await land();
      return { host: nextHost };
    },
    "remount, type, then save ok": async (host) => {
      const nextHost = await remountPendingSave(host);
      typeInto(nextHost, "remounted draft\n");
      await land();
      return { host: nextHost };
    },
    "remount, type, r3, then save ok": async (host) => {
      const nextHost = await remountPendingSave(host);
      typeInto(nextHost, "remounted draft\n");
      await refreshReadme(true);
      await land();
      return { host: nextHost };
    },
    reload: async (host) => {
      host.querySelector(".file-reload").click();
      return { asked: await confirmShown() };
    },
  };

  const row = (state, event, seen, base, cached) => ({ state, event, seen, base, cached });
  const shows = (value, dot, save, status, reload, extra = {}) => ({ value, dot, save, status, reload, ...extra });
  const cachedFile = (revision, text) => ({ revision, content_b64: b64(text) });

  const ROWS = [
    row("clean", "push", shows("moved\n", false, false, "", false), "README.md@ext"),
    row("clean", "switch away and back", shows(README, false, false, "", false, { dotAway: false }), "README.md@1"),
    row("clean", "push while away", shows("moved\n", false, false, "", false, { dotAway: false }), "README.md@ext"),

    row("dirty", "save ok", shows("draft\n", false, false, "", false), "README.md@2"),
    row("dirty", "save refused", shows("draft\n", true, true, CONFLICT, true), "README.md@1"),
    row("dirty", "push", shows("draft\n", true, true, DISK, true), "README.md@1"),
    row("dirty", "switch away and back", shows("draft\n", true, true, "", false, { dotAway: true }), "README.md@1"),
    row("dirty", "save ok while away", shows("draft\n", false, false, "", false, { dotAway: false }), "README.md@2"),
    row("dirty", "push while away", shows("draft\n", true, true, DISK, true, { dotAway: true }), "README.md@1"),

    row("saving", "save ok", shows("draft\n", false, false, "", false), "README.md@2"),
    row("saving", "save refused", shows("draft\n", true, true, CONFLICT, true), "README.md@1"),
    row("saving", "push", shows("draft\n", true, false, DISK, true), null),
    row("saving", "push, then save ok", shows("moved\n", false, false, "", false), "README.md@ext"),
    row("saving", "switch away and back", shows("draft\n", true, false, "", false, { dotAway: true }), null),
    row("saving", "save ok while away", shows("draft\n", false, false, "", false, { dotAway: false }), "README.md@2"),
    row("saving", "push while away", shows("draft\n", true, false, DISK, true, { dotAway: true }), null),
    row("saving", "push, then save ok, while away", shows("moved\n", false, false, "", false, { dotAway: false }), "README.md@ext"),
    row("saving", "same-body refresh, then save ok, while away", shows("draft\n", false, false, "", false, { dotAway: false }), "README.md@2", cachedFile("README.md@2", "draft\n")),
    row("saving", "remount, then save ok", shows("draft\n", false, false, "", false), "README.md@2", cachedFile("README.md@2", "draft\n")),
    row("saving", "remount, type, then save ok", shows("remounted draft\n", true, true, DISK, true), "README.md@1", cachedFile("README.md@2", "draft\n")),
    row("saving", "remount, type, r3, then save ok", shows("remounted draft\n", true, true, DISK, true), "README.md@1", cachedFile("README.md@3", README)),

    row("saving-edited", "save ok", shows(README, true, true, "", false), "README.md@2"),
    row("saving-edited", "save refused", shows(README, false, false, CONFLICT, true), "README.md@1"),
    row("saving-edited", "push", shows(README, true, false, DISK, true), null),
    row("saving-edited", "push, then save ok", shows(README, true, true, DISK, true), "README.md@2"),
    row("saving-edited", "switch away and back", shows(README, true, false, "", false, { dotAway: true }), null),
    row("saving-edited", "save ok while away", shows(README, true, true, "", false, { dotAway: true }), "README.md@2"),
    row("saving-edited", "push, then save ok, while away", shows(README, true, true, DISK, true, { dotAway: true }), "README.md@2"),
    row("saving-edited", "same-body refresh, then save ok, while away", shows(README, true, true, "", false, { dotAway: true }), "README.md@2", cachedFile("README.md@2", "draft\n")),
    row("saving-edited", "r3 with baseline bytes, then save ok, while away", shows(README, true, true, DISK, true, { dotAway: true }), "README.md@2", cachedFile("README.md@3", README)),

    row("changed-on-disk", "save refused", shows("draft\n", true, true, DISK, true), "README.md@1"),
    row("changed-on-disk", "switch away and back", shows("draft\n", true, true, DISK, true, { dotAway: true }), "README.md@1"),
    row("changed-on-disk", "reload", shows(README, false, false, "", false, { asked: true }), "README.md@1"),
  ];

  const mountOnReadme = async () => {
    const { host, files } = mountFiles();
    await openByTap(host, "README.md");
    await openByTap(host, "notes.txt");
    await show(host, "README.md");
    return { host, files };
  };

  const observe = (host) => {
    host.querySelector('[data-file-mode="edit"]').click();
    return {
      value: host.querySelector(".file-editor").value,
      dot: dot(host),
      save: !host.querySelector(".file-save").disabled,
      status: host.querySelector(".file-save-status").textContent,
      reload: !host.querySelector(".file-reload").hidden,
    };
  };

  it.each(ROWS)("$state × $event", async ({ state, event, seen, base, cached }) => {
    let { host } = await mountOnReadme();
    await STATES[state](host);
    const saw = (await EVENTS[event](host)) || {};
    host = saw.host || host;

    const { dotAway, asked, ...painted } = seen;
    await vi.waitFor(() => expect(observe(host)).toEqual(painted));
    if (dotAway !== undefined) expect(saw.dotAway).toBe(dotAway);
    if (asked !== undefined) expect(saw.asked).toBe(asked);
    expect(host.querySelector(".file-editor").value).toBe(seen.value);
    if (cached) expect((await readCached(readmeAddress)).value.file).toMatchObject(cached);

    // The revision the next save goes out against.
    if (base === null) return expect(writes.some((write) => write.open)).toBe(true);
    typeInto(host, `${seen.value}probe\n`);
    await save(host);
    expect(writes.at(-1).params.expected_revision).toBe(base);
  });

  it.each(Object.keys(STATES))("%s × close asks exactly when something is not on disk, and a save landing after leaves nothing behind", async (state) => {
    const { host, files } = await mountOnReadme();
    await STATES[state](host);
    host.querySelector('[data-tab-close="README.md"]').click();
    expect(await confirmShown()).toBe(state !== "clean");
    await vi.waitFor(() => expect(tabNames(host)).toEqual(["notes.txt"]));
    if (writes.some((write) => write.open)) await land();
    expect(tabNames(host)).toEqual(["notes.txt"]);
    expect(files.hasUnsavedChanges()).toBe(false);
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
