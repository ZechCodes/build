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
const { greetBridge, resetChangeEvents } = await import("../src/core/changeEvents.js");
const { BODY_PAGE_BYTES, readBodyPages } = await import("../src/core/bodyPages.js");

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
const mountRoots = ({ roots = ROOTS, openAt = null, onFileOpen = vi.fn(), callRpc = answer } = {}) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const files = renderFilesTab(host, { roots, layoutEntityId: LAYOUT, callRpc, cacheScope: scopeFor("dev-1"), openAt, onFileOpen });
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
  resetChangeEvents();
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

// A file too big for one record (#95) in one of the roots: its pages are read
// by range from that root's own directory, by its path there, and filed under
// that directory's records.
describe("a file over one record in a root", () => {
  const BIG = Array.from({ length: 8000 }, (_, index) => `line ${index + 1} ${"x".repeat(180)}`).join("\n");
  const bigAnswer = (params) => {
    const bytes = Buffer.from(BIG);
    const answered = { path: params.path, size: bytes.length, mime: "text/plain", editable: false, revision: null, truncated: false };
    if (!params.range) return { ...answered, truncated: true, content_b64: bytes.subarray(0, 1024 * 1024).toString("base64") };
    const { offset, bytes: size } = params.range;
    let end = Math.min(bytes.length, offset + size);
    if (end < bytes.length) end = bytes.lastIndexOf(10, end - 1) + 1;
    return { ...answered, content_b64: bytes.subarray(offset, end).toString("base64"), range: { offset, end, total: bytes.length, version: "v1" } };
  };
  const machine = vi.fn(async (method, params) => {
    if (method === "fs.tree" && params.source_id === "assets" && params.path === "") {
      return { path: "", entries: [...TREES.assets[""], { name: "big.log", kind: "file", size: BIG.length }] };
    }
    if (method === "fs.read" && params.path === "big.log") return bigAnswer(params);
    return answer(method, params);
  });
  const ranged = () => machine.mock.calls.filter(([method, params]) => method === "fs.read" && params.range).map(([, params]) => params);

  it("reads its pages from that root's directory, by its path there, into that directory's records", async () => {
    machine.mockClear();
    await greetBridge(async () => ({ api_version: "1.26.0", capabilities: ["bodies.pages"] }), { deviceId: "dev-1" });
    const { host } = mountRoots({ callRpc: machine });
    await tap(host, "assets", "big.log");
    await vi.waitFor(() => expect(host.querySelector(".fpmore")?.hidden).toBe(false));
    expect(ranged()).toEqual([{ workspace_id: "ws-1", source_id: "assets", path: "big.log", range: { offset: 0, bytes: BODY_PAGE_BYTES } }]);
    const head = { deviceId: "dev-1", entityId: directoryCacheId(scopeOf("assets")), kind: "file", sub: "big.log" };
    const [first] = (await readBodyPages(head, "v1")).pages;

    host.querySelector(".fpmore").click();
    await vi.waitFor(async () => expect((await readBodyPages(head, "v1")).pages).toHaveLength(2));
    expect(ranged().at(-1)).toEqual({ workspace_id: "ws-1", source_id: "assets", path: "big.log", range: { offset: first.end, bytes: BODY_PAGE_BYTES } });
  });
});
