// @vitest-environment jsdom
// The Files tab against the local cache. A listing the cache holds is the
// listing: it paints with no round trip, and only a walk into a directory
// nothing has been written for is asked of the machine. A file body is the
// same — opened once, kept under the recent-files rule, and never re-read off
// the wire while the cache holds it.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { scopeFor } = await import("../src/core/cacheScope.js");
const { cachedSubKeys, readCached, writeCached, wipeCache } = await import("../src/core/localCache.js");
const { renderFilesTab } = await import("../src/views/files.js");

const settle = async () => {
  for (let i = 0; i < 20; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};

// The `..` row above a listing is chrome, not an entry: it names neither.
const treeNames = (host) =>
  [...host.querySelectorAll(".fdir, .ffile")].map((row) => (row.dataset.dir || row.dataset.file || "").trim());

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

beforeEach(async () => {
  document.body.innerHTML = "";
  await wipeCache();
});

const mountFiles = (callRpc, { deviceId = "dev-1", ...rest } = {}) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const files = renderFilesTab(host, { scope: { run_id: "run-1" }, callRpc, cacheScope: scopeFor(deviceId), ...rest });
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

    expect(treeNames(host)).toEqual(["a.js"]);
    expect(call.mock.calls.filter(([method]) => method === "fs.tree")).toHaveLength(1);
    const record = await readCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src" });
    expect(record.value.entries[0].name).toBe("a.js");
  });

  it("asks once and no more: walking back into a directory reads what was written", async () => {
    await seedTree("", [{ name: "src", kind: "dir" }]);
    const call = vi.fn(async (method, params) => ({ path: params.path, entries: [{ name: "a.js", kind: "file", size: 2 }] }));
    const { host } = mountFiles(call);
    await settle();
    host.querySelector(".fdir").click();
    await settle();
    host.querySelector(".fup").click();
    await settle();
    host.querySelector(".fdir").click();
    await settle();
    expect(call.mock.calls.filter(([method]) => method === "fs.tree")).toHaveLength(1);
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

  it("leaves a body over a megabyte on screen and off the disk", async () => {
    const twoMegabytes = fileAnswer({ size: 2 * 1024 * 1024, truncated: true, content_b64: b64("hello") });
    const { host } = await openReadme(withReadme(twoMegabytes));
    expect(host.textContent).toContain("truncated");
    expect(await readCached({ deviceId: "dev-1", entityId: "run-1", kind: "file", sub: "README.md" })).toBeUndefined();
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
      [...host.querySelectorAll(".ffile")].find((row) => row.dataset.file === `${name}.md`).click();
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
