// @vitest-environment jsdom
// A workspace's Files (#174): "The file list to have collapsible roots for each
// directory that the workspace has. It's a common IDE pattern we're
// replicating." One tree, one root per directory in the workspace's order, each
// root reading and writing its own directory's records — mounted for real over
// the real cache, answering fs.* per directory from memory.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { scopeFor } = await import("../src/core/cacheScope.js");
const { readCached, wipeCache, writeCached } = await import("../src/core/localCache.js");
const { directoryCacheId } = await import("../src/core/directoryScope.js");
const { renderFilesTab } = await import("../src/views/files.js");

const b64 = (text) => Buffer.from(text, "utf8").toString("base64");

/** Two directories, each with its own tree; both hold an `index.js`. */
const TREES = {
  repo: {
    "": [{ name: "src", kind: "dir" }, { name: "index.js", kind: "file", size: 5 }],
    src: [{ name: "app.js", kind: "file", size: 5 }],
  },
  assets: {
    "": [{ name: "index.js", kind: "file", size: 5 }, { name: "logo.svg", kind: "file", size: 5 }],
  },
};

const answer = vi.fn(async (method, params) => {
  const tree = TREES[params.source_id];
  if (method === "fs.tree") return { path: params.path, entries: tree[params.path] || [] };
  if (method === "fs.read") {
    const text = `${params.source_id}:${params.path}\n`;
    return { path: params.path, mime: "text/plain", size: text.length, truncated: false, editable: true, encoding: "utf-8", revision: "r1", content_b64: b64(text) };
  }
  throw new Error(`unexpected ${method}`);
});

const scopeOf = (sourceId) => ({ workspace_id: "ws-1", source_id: sourceId });
const ROOTS = [
  { id: "repo", label: "Repository", scope: scopeOf("repo") },
  { id: "assets", label: "Assets", scope: scopeOf("assets") },
];
const LAYOUT = "workspace:ws-1";

const mounted = [];
const mountRoots = ({ roots = ROOTS, openAt = null, onFileOpen = vi.fn() } = {}) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const files = renderFilesTab(host, { roots, layoutEntityId: LAYOUT, callRpc: answer, cacheScope: scopeFor("dev-1"), openAt, onFileOpen });
  mounted.push(files);
  return { host, files, onFileOpen };
};
const unmount = (host) => {
  mounted.splice(0).forEach((files) => files.dispose());
  host.remove();
};

const heads = (host) => [...host.querySelectorAll("[data-root-head]")];
const section = (host, id) => [...host.querySelectorAll(".froot")].find((one) => one.dataset.root === id);
const rowsIn = (host, id) => [...section(host, id).querySelectorAll(".frow[data-path]")];
const rowFor = (host, id, path) => rowsIn(host, id).find((row) => row.dataset.path === path);
const tabs = (host) => [...host.querySelectorAll(".ftab-name")].map((tab) => tab.textContent);
const shownPath = (host) => host.querySelector(".fppath")?.textContent;
const treeAddress = (sourceId, dir = "") => ({ deviceId: "dev-1", entityId: directoryCacheId(scopeOf(sourceId)), kind: "tree", sub: dir });
const key = (element, name) => element.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true }));

const drawnBoth = (host) =>
  vi.waitFor(() => {
    expect(rowFor(host, "repo", "index.js")).toBeTruthy();
    expect(rowFor(host, "assets", "logo.svg")).toBeTruthy();
  });

const tap = async (host, id, path) => {
  await vi.waitFor(() => expect(rowFor(host, id, path)).toBeTruthy());
  rowFor(host, id, path).click();
};

beforeEach(async () => {
  document.body.innerHTML = "";
  answer.mockClear();
  window.matchMedia = () => ({ matches: false, addEventListener() {}, removeEventListener() {} });
  await wipeCache();
});

afterEach(() => {
  mounted.splice(0).forEach((files) => files.dispose());
  delete window.matchMedia;
});

describe("one root per directory", () => {
  it("draws a root per directory in the workspace's order, named as the directory, every one expanded", async () => {
    const { host } = mountRoots();
    await drawnBoth(host);
    expect(heads(host).map((head) => [head.textContent.trim(), head.getAttribute("aria-expanded")])).toEqual([
      ["▸Repository", "true"],
      ["▸Assets", "true"],
    ]);
    // One tree; each root's rows stand a level under its row.
    expect(host.querySelectorAll("[role=tree]")).toHaveLength(1);
    expect(heads(host)[0].getAttribute("aria-level")).toBe("1");
    expect(rowFor(host, "repo", "src").getAttribute("aria-level")).toBe("2");
  });

  it("reads each root off its own directory, and files what it reads there", async () => {
    const { host } = mountRoots();
    await drawnBoth(host);
    expect(answer).toHaveBeenCalledWith("fs.tree", { workspace_id: "ws-1", source_id: "repo", path: "" });
    expect(answer).toHaveBeenCalledWith("fs.tree", { workspace_id: "ws-1", source_id: "assets", path: "" });
    await vi.waitFor(async () => {
      expect((await readCached(treeAddress("repo")))?.value.entries.map((entry) => entry.name)).toEqual(["src", "index.js"]);
      expect((await readCached(treeAddress("assets")))?.value.entries.map((entry) => entry.name)).toEqual(["index.js", "logo.svg"]);
    });
  });

  it("moves only the root whose directory a push rewrites", async () => {
    const { host } = mountRoots();
    await drawnBoth(host);
    const assetsRows = rowsIn(host, "assets");
    answer.mockClear();

    await writeCached(treeAddress("repo"), { path: "", entries: [...TREES.repo[""], { name: "NEW.md", kind: "file", size: 1 }] });
    await vi.waitFor(() => expect(rowFor(host, "repo", "NEW.md")).toBeTruthy());

    // The other root's rows are the very nodes they were, and nothing was asked.
    expect(rowsIn(host, "assets")).toEqual(assetsRows);
    expect(assetsRows.every((row) => row.isConnected)).toBe(true);
    expect(answer).not.toHaveBeenCalled();
  });

  it("draws a lone directory as one root, still with its chevron", async () => {
    const { host } = mountRoots({ roots: [ROOTS[1]] });
    await vi.waitFor(() => expect(rowFor(host, "assets", "logo.svg")).toBeTruthy());
    expect(heads(host)).toHaveLength(1);
    expect(heads(host)[0].querySelector(".fchev")).not.toBeNull();
  });
});

describe("folding a root", () => {
  it("folds and unfolds from its row, and the workspace remembers which are folded", async () => {
    const first = mountRoots();
    await drawnBoth(first.host);
    heads(first.host)[0].click();
    await vi.waitFor(() => expect(heads(first.host)[0].getAttribute("aria-expanded")).toBe("false"));
    expect(section(first.host, "repo").querySelector(".froot-list").hidden).toBe(true);
    expect(section(first.host, "assets").querySelector(".froot-list").hidden).toBe(false);
    expect((await readCached({ deviceId: "dev-1", entityId: LAYOUT, kind: "ui-files", sub: "roots" }))?.value)
      .toEqual({ collapsed: ["repo"] });
    unmount(first.host);

    const again = mountRoots();
    await vi.waitFor(() => expect(heads(again.host)[0].getAttribute("aria-expanded")).toBe("false"));
    expect(heads(again.host)[1].getAttribute("aria-expanded")).toBe("true");
    key(heads(again.host)[0], "Enter");
    await vi.waitFor(() => expect(heads(again.host)[0].getAttribute("aria-expanded")).toBe("true"));
    await vi.waitFor(() => expect(rowFor(again.host, "repo", "index.js")).toBeTruthy());
  });
});

describe("files across roots", () => {
  it("opens a file from either root, reading it from its own directory and saying which root it is in", async () => {
    const { host, onFileOpen } = mountRoots();
    await tap(host, "assets", "logo.svg");
    await vi.waitFor(() => expect(shownPath(host)).toBe("logo.svg"));
    expect(answer).toHaveBeenCalledWith("fs.read", { workspace_id: "ws-1", source_id: "assets", path: "logo.svg" });
    expect(onFileOpen).toHaveBeenLastCalledWith("logo.svg", "assets");
    expect(rowFor(host, "assets", "logo.svg").classList.contains("sel")).toBe(true);
  });

  it("names two open files that share a name by their roots", async () => {
    const { host } = mountRoots();
    await tap(host, "repo", "index.js");
    await vi.waitFor(() => expect(tabs(host)).toEqual(["index.js"]));
    await tap(host, "assets", "index.js");
    await vi.waitFor(() => expect(tabs(host)).toEqual(["Repository / index.js", "Assets / index.js"]));
    // The open file is highlighted in its own root only.
    expect(rowFor(host, "assets", "index.js").classList.contains("sel")).toBe(true);
    expect(rowFor(host, "repo", "index.js").classList.contains("sel")).toBe(false);
    // …and the other tab shows the other root's file.
    host.querySelector(".ftab-name").click();
    await vi.waitFor(() => expect(host.querySelector(".fsrc")?.textContent).toContain("repo:index.js"));
  });

  it("opens the file the route names in the root it names, with its directory expanded", async () => {
    const { host } = mountRoots({ openAt: { rootId: "repo", path: "src/app.js" } });
    await vi.waitFor(() => expect(shownPath(host)).toBe("src/app.js"));
    expect(rowFor(host, "repo", "src/app.js").classList.contains("sel")).toBe(true);
    expect(answer).toHaveBeenCalledWith("fs.read", { workspace_id: "ws-1", source_id: "repo", path: "src/app.js" });
  });
});

describe("the keyboard across roots", () => {
  it("walks down from one root's last row to the next root, and back up", async () => {
    const { host } = mountRoots();
    await drawnBoth(host);
    const last = rowFor(host, "repo", "index.js");
    last.focus();
    key(last, "ArrowDown");
    expect(document.activeElement).toBe(heads(host)[1]);
    key(heads(host)[1], "ArrowUp");
    expect(document.activeElement).toBe(rowFor(host, "repo", "index.js"));
    key(document.activeElement, "ArrowUp");
    key(document.activeElement, "ArrowUp");
    expect(document.activeElement).toBe(heads(host)[0]);
    key(heads(host)[0], "ArrowDown");
    expect(document.activeElement).toBe(rowFor(host, "repo", "src"));
  });
});
