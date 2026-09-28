// @vitest-environment jsdom
// The Files tab against the local cache. A listing the cache holds is the
// listing: it paints with no round trip, and only a walk into a directory
// nothing has been written for is asked of the machine. A file body is the
// same — opened once, kept under the recent-files rule, and never re-read off
// the wire while the cache holds it.

import { describe, it, expect, afterEach, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

// Whether the machine's bridge announces `bodies.pages` (#95): a case that
// leaves it off is an older bridge, which refuses `range` on `fs.read`.
let bridgePages = false;
let bridgeMediaRaw = false;
vi.mock("../src/core/changeEvents.js", async (importOriginal) => {
  const actual = await importOriginal();
  return {
    ...actual,
    bridgeCapabilities: (deviceId) => {
      const capabilities = actual.bridgeCapabilities(deviceId);
      return bridgePages ? { ...capabilities, bodies: { pages: true, mediaRawPages: bridgeMediaRaw } } : capabilities;
    },
  };
});

// jsdom has no IntersectionObserver. This one records what it watches, and a
// case scrolls by telling it the sentinel came into view. Like the browser's,
// it reports where a target stands when it starts watching it: out of view,
// unless a case has put the sentinel in view (`sentinelInView`).
const observers = [];
let sentinelInView = false;
let observations = 0;
globalThis.IntersectionObserver = class {
  constructor(callback, options) {
    this.callback = callback;
    this.options = options;
    this.targets = new Set();
    observers.push(this);
  }
  observe(target) {
    this.targets.add(target);
    observations += 1;
    queueMicrotask(() => {
      if (this.targets.has(target)) this.callback([{ target, isIntersecting: sentinelInView }], this);
    });
  }
  unobserve(target) { this.targets.delete(target); }
  disconnect() { this.targets.clear(); }
};

const { scopeFor } = await import("../src/core/cacheScope.js");
const { cachedSubKeys, deleteCached, readCached, writeCached, wipeCache } = await import("../src/core/localCache.js");
const { wipeUiRecords } = await import("../src/core/localUiStore.js");
const { BODY_PAGE_BYTES, dropBodyPages, readBodyPages, writeBodyPage } = await import("../src/core/bodyPages.js");
const { WHOLE_READ } = await import("../src/core/cacheLifetime.js");
const { renderFilesTab } = await import("../src/views/files.js");

const settle = async () => {
  for (let i = 0; i < 20; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};

// Every drawn row, by its path from the checkout's root.
const treeNames = (host) => [...host.querySelectorAll(".fdir, .ffile")].map((row) => row.dataset.path);
const rowFor = (host, path) => [...host.querySelectorAll(".frow[data-path]")].find((row) => row.dataset.path === path);

const b64 = (text) => Buffer.from(text, "utf8").toString("base64");

const fileAnswer = (over = {}) => ({
  path: "README.md",
  size: 5,
  truncated: false,
  mime: "text/plain",
  content_b64: b64("hello"),
  editable: true,
  encoding: "utf-8",
  revision: "r-1",
  ...over,
});

// Every mount of one checkout shares its tab record, so a mount a test left
// standing would follow the next test's tabs and read with the wrong answers.
const mounted = [];

beforeEach(async () => {
  document.body.innerHTML = "";
  bridgePages = false;
  bridgeMediaRaw = false;
  sentinelInView = false;
  observations = 0;
  observers.splice(0);
  await wipeCache();
  await wipeUiRecords();
});

afterEach(() => {
  mounted.splice(0).forEach((files) => files.dispose());
});

const mountFiles = (callRpc, { deviceId = "dev-1", ...rest } = {}) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const files = renderFilesTab(host, { scope: { run_id: "run-1" }, callRpc, cacheScope: scopeFor(deviceId), ...rest });
  mounted.push(files);
  return { host, files };
};

const seedTree = (path, entries, deviceId = "dev-1") =>
  writeCached({ deviceId, entityId: "run-1", kind: "tree", sub: path }, { path, entries });

describe("the cached listing", () => {
  it("is the listing — painted with nothing asked of the machine", async () => {
    await seedTree("", [{ name: "src", kind: "dir" }, { name: "README.md", kind: "file", size: 12 }]);
    const call = vi.fn(async () => ({ path: "", entries: [] }));
    const { host } = mountFiles(call);
    await settle();
    expect(treeNames(host)).toEqual(["src", "README.md"]);
    expect(call).not.toHaveBeenCalled();
  });

  it("moves when a push rewrites the record under it", async () => {
    await seedTree("", [{ name: "old.js", kind: "file", size: 1 }]);
    const { host } = mountFiles(vi.fn(async () => ({ path: "", entries: [] })));
    await settle();
    expect(treeNames(host)).toEqual(["old.js"]);

    await seedTree("", [{ name: "new.js", kind: "file", size: 1 }]);
    await settle();
    expect(treeNames(host)).toEqual(["new.js"]);
  });

  it("does not let a late listing overwrite a newer cache announcement", async () => {
    let answer;
    const call = vi.fn(() => new Promise((resolve) => { answer = resolve; }));
    const { host, files } = mountFiles(call);
    await vi.waitFor(() => expect(answer).toBeTypeOf("function"));

    await seedTree("", [{ name: "newer.js", kind: "file", size: 1 }]);
    await settle();
    answer({ path: "", entries: [{ name: "late.js", kind: "file", size: 1 }] });
    await settle();

    expect(treeNames(host)).toEqual(["newer.js"]);
    expect((await readCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "" })).value.entries[0].name)
      .toBe("newer.js");
    files.dispose();
  });

  it("asks for a directory nothing has been written for, once, and writes it", async () => {
    await seedTree("", [{ name: "src", kind: "dir" }]);
    const call = vi.fn(async (method, params) => ({
      path: params.path,
      entries: [{ name: "a.js", kind: "file", size: 2 }],
    }));
    const { host } = mountFiles(call);
    await settle();

    host.querySelector(".fdir").click();
    await settle();

    expect(treeNames(host)).toEqual(["src", "src/a.js"]);
    expect(call.mock.calls.filter(([method]) => method === "fs.tree")).toHaveLength(1);
    const record = await readCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src" });
    expect(record.value.entries[0].name).toBe("a.js");
  });

  it("asks once and no more: expanding a directory again reads what was written", async () => {
    await seedTree("", [{ name: "src", kind: "dir" }]);
    const call = vi.fn(async (method, params) => ({ path: params.path, entries: [{ name: "a.js", kind: "file", size: 2 }] }));
    const { host } = mountFiles(call);
    await settle();
    host.querySelector(".fdir").click();
    await settle();
    host.querySelector(".fdir").click();
    await settle();
    expect(treeNames(host)).toEqual(["src"]);
    host.querySelector(".fdir").click();
    await settle();
    expect(treeNames(host)).toEqual(["src", "src/a.js"]);
    expect(call.mock.calls.filter(([method]) => method === "fs.tree")).toHaveLength(1);
  });

  it("moves an expanded directory's rows when a push rewrites its record", async () => {
    await seedTree("", [{ name: "src", kind: "dir" }]);
    await seedTree("src", [{ name: "old.js", kind: "file", size: 1 }]);
    const { host } = mountFiles(vi.fn(async () => ({ path: "", entries: [] })));
    await settle();
    host.querySelector(".fdir").click();
    await settle();
    expect(treeNames(host)).toEqual(["src", "src/old.js"]);
    await seedTree("src", [{ name: "new.js", kind: "file", size: 1 }]);
    await settle();
    expect(treeNames(host)).toEqual(["src", "src/new.js"]);
  });

  it("says what went wrong when a directory nobody holds cannot be listed", async () => {
    const { host } = mountFiles(vi.fn(async () => Promise.reject(new Error("unreachable"))));
    await settle();
    expect(host.textContent).toContain("cannot list");
  });

  // The tab is mounted for one machine while the rest of this suite works
  // another: the listing is filed under the machine the view handed it, or a
  // branch on the desktop would paint the laptop's tree.
  it("reads the listing of the machine it was handed", async () => {
    await seedTree("", [{ name: "mine.js", kind: "file", size: 1 }], "dev-1");
    await seedTree("", [{ name: "theirs.js", kind: "file", size: 1 }], "dev-2");
    const { host } = mountFiles(vi.fn(async () => ({ path: "", entries: [] })), { deviceId: "dev-2" });
    await settle();
    expect(treeNames(host)).toEqual(["theirs.js"]);
  });
});

describe("a file body", () => {
  const openReadme = async (call, options) => {
    const mounted = mountFiles(call, options);
    await settle();
    mounted.host.querySelector(".ffile").click();
    await settle();
    return mounted;
  };

  const withReadme = (answer = fileAnswer()) =>
    vi.fn(async (method) => (method === "fs.read" ? answer : { path: "", entries: [] }));

  beforeEach(async () => {
    await seedTree("", [{ name: "README.md", kind: "file", size: 5 }]);
  });

  it("is written where the reader opened it, and stamped with when", async () => {
    const { host } = await openReadme(withReadme());
    expect(host.textContent).toContain("hello");
    const record = await readCached({ deviceId: "dev-1", entityId: "run-1", kind: "file", sub: "README.md" });
    expect(record.value.file.revision).toBe("r-1");
    expect(typeof record.value.openedAt).toBe("number");
  });

  it("opens from the cache with no fs.read at all", async () => {
    await writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "file", sub: "README.md" },
      { file: fileAnswer({ content_b64: b64("from the disk") }), openedAt: Date.now() },
    );
    const call = withReadme();
    const { host } = await openReadme(call);
    expect(host.textContent).toContain("from the disk");
    expect(call.mock.calls.filter(([method]) => method === "fs.read")).toHaveLength(0);
  });

  it("opens when another writer fills the selected file record and the pull never answers", async () => {
    const call = vi.fn((method) => method === "fs.read" ? new Promise(() => {}) : Promise.resolve({ path: "", entries: [] }));
    const { host, files } = mountFiles(call);
    await settle();
    host.querySelector(".ffile").click();
    await settle();

    await writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "file", sub: "README.md" },
      { file: fileAnswer({ content_b64: b64("from the announcement") }), openedAt: Date.now() },
    );
    await settle();

    expect(host.textContent).toContain("from the announcement");
    files.dispose();
  });

  it("does not let a late read overwrite a newer selected-file record", async () => {
    let answer;
    const call = vi.fn((method) => method === "fs.read"
      ? new Promise((resolve) => { answer = resolve; })
      : Promise.resolve({ path: "", entries: [] }));
    const { host, files } = mountFiles(call);
    await settle();
    host.querySelector(".ffile").click();
    await vi.waitFor(() => expect(answer).toBeTypeOf("function"));

    const newer = fileAnswer({ revision: "r-2", content_b64: b64("newer announcement") });
    await writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "file", sub: "README.md" },
      { file: newer, openedAt: Date.now() },
    );
    answer(fileAnswer({ content_b64: b64("late pull") }));
    await settle();

    expect(host.textContent).toContain("newer announcement");
    expect(host.textContent).not.toContain("late pull");
    expect((await readCached({ deviceId: "dev-1", entityId: "run-1", kind: "file", sub: "README.md" })).value.file.revision)
      .toBe("r-2");
    files.dispose();
  });

  it("holds a cache repaint while the editor has a newer local draft", async () => {
    const { host, files } = await openReadme(withReadme());
    host.querySelector('[data-file-mode="edit"]').click();
    const editor = host.querySelector(".file-editor");
    editor.value = "local draft";
    editor.dispatchEvent(new Event("input", { bubbles: true }));

    await writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "file", sub: "README.md" },
      { file: fileAnswer({ revision: "r-2", content_b64: b64("disk change") }), openedAt: Date.now() },
    );
    await settle();

    expect(host.querySelector(".file-editor").value).toBe("local draft");
    expect(host.textContent).toContain("File changed on disk");
    files.dispose();
  });

  it("completes a save from the file record written with the fs.write result", async () => {
    const call = vi.fn(async (method, params) => {
      if (method === "fs.read") return fileAnswer();
      if (method === "fs.write") {
        return fileAnswer({ revision: "r-2", size: 12, content_b64: params.content_b64 });
      }
      return { path: "", entries: [] };
    });
    const { host, files } = await openReadme(call);
    host.querySelector('[data-file-mode="edit"]').click();
    const editor = host.querySelector(".file-editor");
    editor.value = "saved value";
    editor.dispatchEvent(new Event("input", { bubbles: true }));
    host.querySelector(".file-save").click();
    await settle();

    const record = await readCached({ deviceId: "dev-1", entityId: "run-1", kind: "file", sub: "README.md" });
    expect(record.value.file.revision).toBe("r-2");
    expect(record.value.file.content_b64).toBe(b64("saved value"));
    expect(host.querySelector(".file-editor").value).toBe("saved value");
    expect(host.querySelector(".file-save").disabled).toBe(true);
    files.dispose();
  });

  it("re-stamps what it opened off the disk, so the five kept are the five last read", async () => {
    const longAgo = Date.now() - 3600 * 1000;
    await writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "file", sub: "README.md" },
      { file: fileAnswer({ content_b64: b64("from the disk") }), openedAt: longAgo },
    );
    const call = withReadme();
    const { host } = await openReadme(call);
    expect(host.textContent).toContain("from the disk");
    expect(call.mock.calls.filter(([method]) => method === "fs.read")).toHaveLength(0);
    const record = await readCached({ deviceId: "dev-1", entityId: "run-1", kind: "file", sub: "README.md" });
    // Opening a file is what makes it recent, whichever side it came from —
    // otherwise the one the reader keeps coming back to is the one evicted.
    expect(record.value.openedAt).toBeGreaterThan(longAgo);
    expect(record.value.file.content_b64).toBe(b64("from the disk"));
  });

  // A whole answer is weighed by the file's own size, not by what it carried:
  // one the file's size puts over the cap is kept as pages all the same, and
  // painted from them.
  it("keeps a whole body the file's own size puts over the cap as pages, and paints them", async () => {
    const wholeAndHuge = fileAnswer({ size: 2 * 1024 * 1024, content_b64: b64("hello") });
    const { host } = await openReadme(withReadme(wholeAndHuge));
    expect(host.textContent).toContain("hello");
    const record = await readCached({ deviceId: "dev-1", entityId: "run-1", kind: "file", sub: "README.md" });
    expect(record.value.file).toMatchObject({ paged: true, of: WHOLE_READ });
    expect(record.value.file.content_b64).toBeUndefined();
  });

  it("keeps the five most recently opened and no more", async () => {
    const names = ["a", "b", "c", "d", "e", "f", "g"];
    await seedTree("", names.map((name) => ({ name: `${name}.md`, kind: "file", size: 5 })));
    const call = vi.fn(async (method, params) =>
      method === "fs.read" ? fileAnswer({ path: params.path, content_b64: b64(params.path) }) : { path: "", entries: [] },
    );
    const { host } = mountFiles(call);
    await settle();
    for (const name of names) {
      rowFor(host, `${name}.md`).click();
      await settle();
    }
    expect((await cachedSubKeys("dev-1", "run-1", "file")).sort()).toEqual(["c.md", "d.md", "e.md", "f.md", "g.md"]);
  });
});

// A workspace source is filed under an id no sync pass ever writes: its git
// subjects are runs, projects and external worktrees, never a durable
// workspace id, so `listTrees` and `rereadHeldFiles` never reach it. There the
// record is the last paint's own work, and this tab is the only reader — so it
// is a seed, and every walk and every open still goes to the machine.
describe("a checkout nothing walks", () => {
  const SOURCE = { workspace_id: "w-1", source_id: "s-1" };
  const SOURCE_ENTITY = `workspace:${JSON.stringify(["w-1", "s-1"])}`;

  const seedSourceTree = (path, entries) =>
    writeCached({ deviceId: "dev-1", entityId: SOURCE_ENTITY, kind: "tree", sub: path }, { path, entries });

  it("re-lists a directory it already holds, because nothing else keeps it true", async () => {
    await seedSourceTree("", [{ name: "a.js", kind: "file", size: 1 }]);
    const call = vi.fn(async (method, params) => ({
      path: params.path,
      entries: [{ name: "a.js", kind: "file", size: 1 }, { name: "new.js", kind: "file", size: 1 }],
    }));
    const { host } = mountFiles(call, { scope: SOURCE });
    await settle();

    expect(call.mock.calls.filter(([method]) => method === "fs.tree")).toHaveLength(1);
    expect(treeNames(host)).toEqual(["a.js", "new.js"]);
  });

  it("re-reads a body it already holds, so an edited file is not the one it was", async () => {
    await seedSourceTree("", [{ name: "README.md", kind: "file", size: 5 }]);
    await writeCached(
      { deviceId: "dev-1", entityId: SOURCE_ENTITY, kind: "file", sub: "README.md" },
      { file: fileAnswer({ content_b64: b64("before the edit") }), openedAt: Date.now() },
    );
    const call = vi.fn(async (method, params) =>
      method === "fs.read"
        ? fileAnswer({ content_b64: b64("after the edit") })
        : { path: params.path, entries: [{ name: "README.md", kind: "file", size: 5 }] },
    );
    const { host } = mountFiles(call, { scope: SOURCE });
    await settle();
    host.querySelector(".ffile").click();
    await settle();

    expect(host.textContent).toContain("after the edit");
    expect(call.mock.calls.filter(([method]) => method === "fs.read")).toHaveLength(1);
  });

  it("keeps a body that has grown past one record as pages of what the machine sent", async () => {
    await seedSourceTree("", [{ name: "README.md", kind: "file", size: 5 }]);
    await writeCached(
      { deviceId: "dev-1", entityId: SOURCE_ENTITY, kind: "file", sub: "README.md" },
      { file: fileAnswer({ content_b64: b64("small, once") }), openedAt: Date.now() },
    );
    const grown = fileAnswer({ size: 2 * 1024 * 1024, truncated: true, content_b64: b64("the first megabyte\n") });
    const call = vi.fn(async (method, params) =>
      method === "fs.read" ? grown : { path: params.path, entries: [{ name: "README.md", kind: "file", size: 5 }] },
    );
    const { host } = mountFiles(call, { scope: SOURCE });
    await settle();
    host.querySelector(".ffile").click();
    await settle();

    expect(host.textContent).toContain("the first megabyte");
    expect(host.querySelector(".fpmore").hidden).toBe(false);
    expect(host.textContent).not.toContain("small, once");
    // Not "keep the old one": the record is of the file as it is now, the piece
    // of it the machine sent, and says it is a piece.
    const record = await readCached({ deviceId: "dev-1", entityId: SOURCE_ENTITY, kind: "file", sub: "README.md" });
    expect(record.value.file).toMatchObject({ paged: true, truncated: true });
  });

  it("paints what it holds before the machine answers", async () => {
    await seedSourceTree("", [{ name: "held.js", kind: "file", size: 1 }]);
    let answer = null;
    const call = vi.fn(
      (method, params) => new Promise((resolve) => (answer = () => resolve({ path: params.path, entries: [] }))),
    );
    const { host } = mountFiles(call, { scope: SOURCE });
    await settle();
    expect(treeNames(host)).toEqual(["held.js"]);
    answer();
    await settle();
    expect(treeNames(host)).toEqual([]);
  });
});

// A file over one record is kept as pages beside a record saying what it is
// (#95). Every paint comes from those records: the first page is read by
// range when the file is opened, the next when the reader scrolls to the end
// of what is painted, and a revisit reads nothing.
describe("a file over one record", () => {
  const TEXT_CAP = 1024 * 1024;
  const MEDIA_CAP = 64 * 1024 * 1024;
  const line = (number) => `line ${number} ${"x".repeat(180)}`;
  const bigText = (lines, from = 1) => Array.from({ length: lines }, (_, index) => line(from + index)).join("\n");

  /** A machine holding one file, cut the way the bridge cuts it: a whole read
   *  capped, a ranged one ending after its last whole line. */
  const machine = (file) => vi.fn(async (method, params) => {
    if (method !== "fs.read") return { path: params.path, entries: [] };
    const bytes = Buffer.from(file.bytes);
    const media = ["image/", "audio/", "video/"].some((prefix) => file.mime.startsWith(prefix));
    const answer = { path: params.path, size: bytes.length, mime: file.mime, editable: false, revision: null };
    if (!params.range) {
      const cap = media ? MEDIA_CAP : TEXT_CAP;
      return { ...answer, truncated: bytes.length > cap, content_b64: bytes.subarray(0, cap).toString("base64") };
    }
    const { offset, bytes: size } = params.range;
    let end = Math.min(bytes.length, offset + size);
    const newline = bytes.lastIndexOf(10, end - 1);
    if (end < bytes.length && !media && newline >= offset) end = newline + 1;
    return {
      ...answer,
      truncated: false,
      content_b64: bytes.subarray(offset, end).toString("base64"),
      range: { offset, end, total: bytes.length, version: file.version },
    };
  });

  const reads = (call) => call.mock.calls.filter(([method]) => method === "fs.read").map(([, params]) => params);
  const lineNumbers = (host) => [...host.querySelectorAll(".fsrc tr[data-new-line]")].map((row) => Number(row.dataset.newLine));
  const head = (path) => ({ deviceId: "dev-1", entityId: "run-1", kind: "file", sub: path });

  const scrollToSentinel = async (host) => {
    const sentinel = host.querySelector(".fpmore");
    for (const observer of observers) {
      if (observer.targets.has(sentinel)) observer.callback([{ target: sentinel, isIntersecting: true }], observer);
    }
    await settle();
  };

  const open = async (call, path = "big.log") => {
    const mounted = mountFiles(call);
    await settle();
    rowFor(mounted.host, path).click();
    await settle();
    return mounted;
  };

  beforeEach(async () => {
    await seedTree("", [
      { name: "big.log", kind: "file", size: 2 * TEXT_CAP },
      { name: "shot.png", kind: "file", size: 3 * TEXT_CAP },
    ]);
  });

  it("is kept as a record and its first page, read by range, and painted from them", async () => {
    bridgePages = true;
    const file = { bytes: bigText(8000), mime: "text/plain", version: "v1" };
    const call = machine(file);
    const { host } = await open(call);

    expect(reads(call)).toEqual([
      { run_id: "run-1", path: "big.log" },
      { run_id: "run-1", path: "big.log", range: { offset: 0, bytes: BODY_PAGE_BYTES } },
    ]);
    const record = await readCached(head("big.log"));
    expect(record.value.file).toMatchObject({ paged: true, of: "v1", size: Buffer.byteLength(file.bytes), editable: false });
    const held = await readBodyPages(head("big.log"), "v1");
    expect(held.pages).toHaveLength(1);

    const numbers = lineNumbers(host);
    expect(numbers[0]).toBe(1);
    expect(numbers.length).toBe(Buffer.from(held.pages[0].body, "base64").toString().split("\n").length - 1);
    expect(host.querySelector(".fsrc").textContent).toContain(line(numbers.length));
    expect(host.querySelector(".fpmore").hidden).toBe(false);
    expect(host.querySelector(".fpmore").textContent).toMatch(/^Showing .+ of .+MB$/);
    expect(host.querySelector('[data-file-mode="edit"]')).toBeNull();
  });

  it("paints again from the cache with nothing asked of the machine", async () => {
    bridgePages = true;
    const file = { bytes: bigText(8000), mime: "text/plain", version: "v1" };
    const first = await open(machine(file));
    const painted = lineNumbers(first.host);
    first.files.dispose();
    first.host.remove();

    const call = machine(file);
    const { host } = await open(call);
    expect(reads(call)).toEqual([]);
    expect(lineNumbers(host)).toEqual(painted);
  });

  it("reads the next page when the reader reaches the sentinel, and numbers its lines on", async () => {
    bridgePages = true;
    const file = { bytes: bigText(8000), mime: "text/plain", version: "v1" };
    const call = machine(file);
    const { host } = await open(call);
    const firstRow = host.querySelector(".fsrc tr");
    const firstPage = lineNumbers(host).length;
    const { end } = (await readBodyPages(head("big.log"), "v1")).pages[0];

    await scrollToSentinel(host);

    expect(reads(call).at(-1)).toEqual({ run_id: "run-1", path: "big.log", range: { offset: end, bytes: BODY_PAGE_BYTES } });
    expect((await readBodyPages(head("big.log"), "v1")).pages).toHaveLength(2);
    const numbers = lineNumbers(host);
    expect(numbers.length).toBeGreaterThan(firstPage);
    expect(numbers).toEqual(numbers.map((_, index) => index + 1));
    expect(host.querySelector(`.fsrc tr[data-new-line="${firstPage + 1}"]`).textContent).toContain(line(firstPage + 1));
    // Appended under the rows already painted, not painted over them.
    expect(host.querySelector(".fsrc tr")).toBe(firstRow);
  });

  it("reads to the end, and says no more once it is there", async () => {
    bridgePages = true;
    const file = { bytes: bigText(7000), mime: "text/plain", version: "v1" };
    const call = machine(file);
    const { host } = await open(call);
    for (let turn = 0; turn < 8 && !host.querySelector(".fpmore").hidden; turn += 1) await scrollToSentinel(host);

    expect(host.querySelector(".fpmore").hidden).toBe(true);
    expect(lineNumbers(host)).toEqual(Array.from({ length: 7000 }, (_, index) => index + 1));
    expect(host.querySelector(".ftrunc").hidden).toBe(true);
  });

  // A file of one line longer than any page — minified, generated — is shown
  // a page at a time as one row, never held back until its end. A page that
  // only carries that line on leaves the sentinel in view, so it is not what
  // reads the next: the reader going along the line is, or pressing it
  // (#95 round 2).
  it("paints a line longer than a page as far as it has come, and reads on only as the reader goes along it", async () => {
    bridgePages = true;
    sentinelInView = true;
    const file = { bytes: "x".repeat(2 * TEXT_CAP), mime: "text/plain", version: "v1" };
    const call = machine(file);
    const { host } = await open(call);
    const ranged = () => reads(call).filter((params) => params.range);
    const lineText = () => host.querySelector('.fsrc tr[data-new-line="1"] code').textContent;

    expect(ranged()).toHaveLength(2);
    expect(lineNumbers(host)).toEqual([1]);
    expect(lineText()).toBe("x".repeat(2 * BODY_PAGE_BYTES));
    await settle();
    expect(ranged()).toHaveLength(2);

    host.querySelector(".fsrc").dispatchEvent(new window.Event("scroll"));
    await settle();
    expect(ranged()).toHaveLength(3);
    expect(lineNumbers(host)).toEqual([1]);
    expect(lineText()).toHaveLength(3 * BODY_PAGE_BYTES);

    host.querySelector(".fpmore").click();
    await settle();
    expect(ranged()).toHaveLength(4);
    expect(lineText()).toHaveLength(4 * BODY_PAGE_BYTES);
    expect(host.querySelector(".fpmore").textContent).toBe("Showing 1.0 MB of 2.1 MB");
  });

  // The view says what it holds, whatever the machine can do: how much of the
  // file is painted. It never asks whether the bridge can page.
  it("never sends an older bridge a range, and keeps what it sent as pages that say how much they are", async () => {
    const file = { bytes: bigText(8000), mime: "text/plain", version: "v1" };
    const call = machine(file);
    const { host } = await open(call);
    sentinelInView = true;
    await scrollToSentinel(host);
    const watched = observations;
    await scrollToSentinel(host);

    expect(reads(call)).toEqual([{ run_id: "run-1", path: "big.log" }]);
    const record = await readCached(head("big.log"));
    expect(record.value.file).toMatchObject({ paged: true, of: WHOLE_READ, truncated: true });
    const held = await readBodyPages(head("big.log"), WHOLE_READ);
    expect(held.end).toBe(TEXT_CAP);
    expect(held.complete).toBe(false);
    expect(host.querySelector(".fpmore").hidden).toBe(false);
    expect(host.querySelector(".fpmore").textContent).toBe("Showing 1.0 MB of 1.5 MB");
    expect(host.querySelector(".ftrunc").hidden).toBe(true);
    expect(lineNumbers(host)[0]).toBe(1);
    // A read that cannot be made paints nothing, so the sentinel is not
    // watched again and asked again: no loop.
    expect(observations).toBe(watched);
  });

  // Before its machine's greeting every capability reads off. A body painted
  // from the cache then must not be painted as cut for good: once greeted, a
  // write to its pages (the sync layer's refresh) repaints it, and the
  // sentinel still in view reads on, with no remount.
  it("mounted before the greeting, says how much it holds and reads on once greeted", async () => {
    bridgePages = true;
    const file = { bytes: bigText(8000), mime: "text/plain", version: "v1" };
    (await open(machine(file))).host.remove();
    mounted.splice(0).forEach((files) => files.dispose());
    bridgePages = false;

    const call = machine(file);
    const { host } = await open(call);
    sentinelInView = true;
    await scrollToSentinel(host);
    expect(reads(call)).toEqual([]);
    expect(host.querySelector(".fpmore").hidden).toBe(false);
    expect(host.querySelector(".fpmore").textContent).toMatch(/^Showing .+ of .+MB$/);
    expect(host.querySelector(".ftrunc").hidden).toBe(true);
    const firstRow = host.querySelector(".fsrc tr");
    const [firstPage] = (await readBodyPages(head("big.log"), "v1")).pages;

    bridgePages = true;
    await writeBodyPage(head("big.log"), firstPage);
    await settle();

    expect(reads(call)[0]).toEqual({ run_id: "run-1", path: "big.log", range: { offset: firstPage.end, bytes: BODY_PAGE_BYTES } });
    expect((await readBodyPages(head("big.log"), "v1")).pages.length).toBeGreaterThan(1);
    expect(host.querySelector(".fsrc tr")).toBe(firstRow);
  });

  // The recent-files rule, or a newer store, can let go of the record while a
  // page is being read: that page is of nothing any more, and not kept.
  it("does not keep a page that lands after its file's record was let go of", async () => {
    bridgePages = true;
    const file = { bytes: bigText(8000), mime: "text/plain", version: "v1" };
    const call = machine(file);
    const { host } = await open(call);
    await deleteCached([head("big.log")]);
    await dropBodyPages(head("big.log"));

    await scrollToSentinel(host);

    expect(reads(call).at(-1).range.offset).toBeGreaterThan(0);
    expect(await cachedSubKeys("dev-1", "run-1", "page")).toEqual([]);
  });

  // A first page is a wire call: a record written while it was out — a push's
  // refresh, another tab — is left as it is, with its pages (#95 round 4).
  it("keeps no first page, nor its record, over a newer record written while the page was read", async () => {
    bridgePages = true;
    const file = { bytes: bigText(8000), mime: "text/plain", version: "v1" };
    const answered = machine(file);
    let releaseFirstPage = null;
    const call = vi.fn(async (method, params) => {
      if (params.range?.offset === 0) await new Promise((resolve) => { releaseFirstPage = resolve; });
      return answered(method, params);
    });
    const newerPage = { of: "v2", offset: 0, end: 8, total: 8, body: b64("newer!\n\n") };
    const newer = { path: "big.log", mime: "text/plain", size: 8, editable: false, revision: null, paged: true, of: "v2" };

    await open(call);
    await vi.waitFor(() => expect(releaseFirstPage).toBeTypeOf("function"));
    await writeBodyPage(head("big.log"), newerPage);
    await writeCached(head("big.log"), { file: newer, openedAt: Date.now() });
    releaseFirstPage();
    await settle();

    expect((await readCached(head("big.log"))).value.file).toEqual(newer);
    expect((await readBodyPages(head("big.log"), "v2")).pages).toEqual([newerPage]);
    expect(await cachedSubKeys("dev-1", "run-1", "page")).toEqual(["file:big.log@0"]);
  });

  it("paints a file emptied under it as the one empty line it is, and reads no more", async () => {
    bridgePages = true;
    const file = { bytes: bigText(8000), mime: "text/plain", version: "v1" };
    const call = machine(file);
    const { host } = await open(call);
    file.bytes = "";
    file.version = "v2";

    await scrollToSentinel(host);

    const record = await readCached(head("big.log"));
    expect(record.value.file).toMatchObject({ paged: true, of: "v2", size: 0 });
    expect((await readBodyPages(head("big.log"), "v2")).complete).toBe(true);
    expect(lineNumbers(host)).toEqual([1]);
    expect(host.querySelector(".fpmore").hidden).toBe(true);
  });

  // A file an agent keeps writing to is of a new version at every read. The
  // view starts over from the first page when the next is of a changed file,
  // but not again off its own start-over: the next goes with the next change
  // somebody else wrote down (a push's refresh), so it cannot spin.
  it("starts over once off its own reading, then waits for a record it did not write", async () => {
    bridgePages = true;
    let version = 0;
    const file = { bytes: bigText(8000), mime: "text/plain", get version() { version += 1; return `v${version}`; } };
    const call = machine(file);
    const { host } = await open(call);
    await scrollToSentinel(host);
    const afterFirst = reads(call).length;
    const restarted = (await readCached(head("big.log"))).value.file.of;

    await scrollToSentinel(host);
    await scrollToSentinel(host);

    expect(reads(call).length - afterFirst).toBe(2);
    expect(reads(call).slice(afterFirst).every((params) => params.range.offset > 0)).toBe(true);
    expect((await readCached(head("big.log"))).value.file.of).toBe(restarted);
  });

  it("starts over from the first page when the next one is of a changed file", async () => {
    bridgePages = true;
    const file = { bytes: bigText(8000), mime: "text/plain", version: "v1" };
    const call = machine(file);
    const { host } = await open(call);

    file.bytes = bigText(8000, 100_001);
    file.version = "v2";
    await scrollToSentinel(host);

    expect(reads(call).at(-1)).toEqual({ run_id: "run-1", path: "big.log", range: { offset: 0, bytes: BODY_PAGE_BYTES } });
    expect((await readCached(head("big.log"))).value.file.of).toBe("v2");
    expect((await readBodyPages(head("big.log"), "v1")).pages).toHaveLength(0);
    expect(lineNumbers(host)[0]).toBe(1);
    expect(host.querySelector(".fsrc tr").textContent).toContain(line(100_001));
  });

  it("reads a picture over the cap whole into pages, and paints it from them", async () => {
    bridgePages = true;
    const file = { bytes: "p".repeat(3 * TEXT_CAP + 7), mime: "image/png", version: "v1" };
    const call = machine(file);
    const { host } = await open(call, "shot.png");

    expect(reads(call).slice(1).map((params) => params.range)).toEqual([0, 1, 2, 3].map((index) =>
      ({ offset: index * TEXT_CAP, bytes: TEXT_CAP })));
    const held = await readBodyPages(head("shot.png"), "v1");
    expect(held.complete).toBe(true);
    expect(held.pages).toHaveLength(4);
    const image = host.querySelector("img.fimg");
    expect(image.getAttribute("src")).toMatch(/^blob:/);
    expect(host.querySelector(".fpmore").hidden).toBe(true);
  });

  it("opens video from background raw byte pages and revokes its Blob URL on navigation", async () => {
    bridgePages = true;
    bridgeMediaRaw = true;
    await seedTree("", [{ name: "clip.mp4", kind: "file", size: 2 * TEXT_CAP }, { name: "big.log", kind: "file", size: 2 * TEXT_CAP }]);
    const file = { bytes: "v".repeat(2 * TEXT_CAP + 7), mime: "video/mp4", version: "v1" };
    const call = machine(file);
    const revoke = vi.spyOn(URL, "revokeObjectURL");
    const { host } = await open(call, "clip.mp4");
    const video = host.querySelector("video.fmedia");
    expect(video?.getAttribute("src")).toMatch(/^blob:/);
    const address = video.getAttribute("src");
    expect(reads(call)[0].range).toEqual({ offset: 0, bytes: TEXT_CAP, raw: true });
    expect(reads(call).filter((params) => params.path === "clip.mp4").every((params) => params.range.raw)).toBe(true);
    expect(call.mock.calls.filter(([method, params]) => method === "fs.read" && params.path === "clip.mp4")
      .every(([, , options]) => options?.priority === "background")).toBe(true);
    rowFor(host, "big.log").click();
    await vi.waitFor(() => expect(revoke).toHaveBeenCalledWith(address));
    revoke.mockRestore();
  });
});
