// The owner's lifetime rules for what a workspace holds: everything but the
// feed row goes at once when the workspace is finished or deleted, and ages
// out 72 h after its last write once the workspace is only recent. The lists
// (devices, projects, workspaces, feed) are nobody's workspace data and are
// replaced, never expired.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const NOW = 1_800_000_000_000;
const HOUR = 60 * 60 * 1000;

let cache;
let lifetime;
let pages;

/** A record written as if at a given moment: `at` is the TTL clock. */
const writeAt = async (address, value, at) => {
  const clock = vi.spyOn(Date, "now").mockReturnValue(at);
  await cache.writeCached(address, value);
  clock.mockRestore();
};

const held = async (address) => (await cache.readCached(address)) !== undefined;

const address = (entityId, kind, sub = "") => ({ deviceId: "dev-1", entityId, kind, sub });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  lifetime = await import("../src/core/cacheLifetime.js");
  pages = await import("../src/core/bodyPages.js");
});

describe("the 72 h expiry", () => {
  it("is 72 h", () => {
    expect(lifetime.WORKSPACE_DATA_TTL_MS).toBe(72 * HOUR);
  });

  it("drops the workspace data last written more than 72 h ago, and keeps the rest", async () => {
    await writeAt(address("ws-1", "status"), { head: "old" }, NOW - 73 * HOUR);
    await writeAt(address("ws-1", "thread", "agent-1"), { items: [] }, NOW - 100 * HOUR);
    await writeAt(address("ws-1", "tree", "src"), { entries: [] }, NOW - 72 * HOUR); // not yet older than
    await writeAt(address("ws-1", "log"), { commits: [] }, NOW - HOUR);
    await writeAt(address("ws-1", "row"), { kind: "workspace" }, NOW - 100 * HOUR);
    await writeAt(address("", "feed"), { items: [] }, NOW - 100 * HOUR);
    await writeAt(address("", "projects"), [], NOW - 100 * HOUR);

    await lifetime.expireWorkspaceData("dev-1", "ws-1", NOW);

    expect(await held(address("ws-1", "status"))).toBe(false);
    expect(await held(address("ws-1", "thread", "agent-1"))).toBe(false);
    expect(await held(address("ws-1", "tree", "src"))).toBe(true);
    expect(await held(address("ws-1", "log"))).toBe(true);
    // The row belongs to the feed, which decides for itself what it lists.
    expect(await held(address("ws-1", "row"))).toBe(true);
    expect(await held(address("", "feed"))).toBe(true);
    expect(await held(address("", "projects"))).toBe(true);
  });

  it("leaves a neighbouring workspace's stale data alone", async () => {
    await writeAt(address("ws-1", "status"), {}, NOW - 100 * HOUR);
    await writeAt(address("ws-2", "status"), {}, NOW - 100 * HOUR);
    await lifetime.expireWorkspaceData("dev-1", "ws-1", NOW);
    expect(await held(address("ws-2", "status"))).toBe(true);
  });

  it("answers the addresses it dropped, and tells the surfaces holding them", async () => {
    const heard = [];
    cache.subscribeCache({ deviceId: "dev-1", entityId: "ws-1" }, (changed) => heard.push(changed.kind));
    await writeAt(address("ws-1", "status"), {}, NOW - 100 * HOUR);
    await writeAt(address("ws-1", "log"), {}, NOW);
    const dropped = await lifetime.expireWorkspaceData("dev-1", "ws-1", NOW);
    expect(dropped.map((one) => one.kind)).toEqual(["status"]);
    expect(heard).toEqual(["status", "log", "status"]); // the two writes, then the drop
  });

  it("does nothing, and asks for nothing, when a workspace holds no stale data", async () => {
    await writeAt(address("ws-1", "status"), {}, NOW);
    expect(await lifetime.expireWorkspaceData("dev-1", "ws-1", NOW)).toEqual([]);
    expect(await held(address("ws-1", "status"))).toBe(true);
  });
});

describe("done and deleted", () => {
  it("keeps restored data when an old eviction finishes enumerating after its owner is gone", async () => {
    const status = address("ws-1", "status");
    await writeAt(status, { head: "old" }, NOW);
    const originalAddresses = cache.cachedAddresses;
    let release;
    const enumeration = vi.spyOn(cache, "cachedAddresses").mockImplementation((scope) => {
      enumeration.mockRestore();
      return new Promise((resolve) => {
        release = async () => resolve(await originalAddresses(scope));
      });
    });
    let active = true;
    const pending = lifetime.evictWorkspaceData("dev-1", "ws-1", () => active);
    await vi.waitFor(() => expect(release).toBeTypeOf("function"));

    active = false;
    await cache.writeCached(status, { head: "restored" });
    await release();
    await pending;
    expect((await cache.readCached(status))?.value).toEqual({ head: "restored" });
  });

  it("drops every kind that workspace holds, however fresh, and leaves the row", async () => {
    const kinds = ["status", "log", "unpushed", "diff", "terminals", "console", "surfaces"];
    for (const kind of kinds) await writeAt(address("ws-1", kind), { kind }, NOW);
    await writeAt(address("ws-1", "patch", "abc123"), {}, NOW);
    await writeAt(address("ws-1", "file", "src/a.js"), { content: "a" }, NOW);
    await writeAt(address("ws-1", "row"), { kind: "workspace" }, NOW);

    await lifetime.evictWorkspaceData("dev-1", "ws-1");

    for (const kind of kinds) expect(await held(address("ws-1", kind))).toBe(false);
    expect(await held(address("ws-1", "patch", "abc123"))).toBe(false);
    expect(await held(address("ws-1", "file", "src/a.js"))).toBe(false);
    expect(await held(address("ws-1", "row"))).toBe(true);
  });

  it("takes nothing from a neighbour, or from the lists", async () => {
    await writeAt(address("ws-1", "status"), {}, NOW);
    await writeAt(address("ws-2", "status"), {}, NOW);
    await writeAt(address("ws-12", "status"), {}, NOW); // the id ws-1 is a prefix of
    await writeAt(address("", "workspaces"), [], NOW);
    await lifetime.evictWorkspaceData("dev-1", "ws-1");
    expect(await held(address("ws-2", "status"))).toBe(true);
    expect(await held(address("ws-12", "status"))).toBe(true);
    expect(await held(address("", "workspaces"))).toBe(true);
  });
});

describe("what a sweep reads", () => {
  /** Watch the ways a record body can leave the store. `getAll`, `get` and a
   *  value cursor each deserialize one; `getAllKeys` and an index key cursor
   *  do not. The spies sit on the store's prototype, so they see every
   *  transaction the module opens. */
  const watchBodyReads = async () => {
    await cache.readCached(address("ws-1", "status")); // the module opens its connection
    const db = await new Promise((resolve, reject) => {
      const request = indexedDB.open("build-cache");
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    const prototype = Object.getPrototypeOf(db.transaction("records", "readonly").objectStore("records"));
    db.close();
    const read = [];
    const spies = ["get", "getAll", "openCursor"].map((method) => {
      const original = prototype[method];
      return vi.spyOn(prototype, method).mockImplementation(function spy(...args) {
        read.push(method);
        return original.apply(this, args);
      });
    });
    return { read, stop: () => spies.forEach((spy) => spy.mockRestore()) };
  };

  it("never deserializes a record body to decide what has aged out or must go", async () => {
    // A workspace holds up to five file bodies of 1 MB, a 256 KB working-tree
    // diff and twenty patches. A sweep that read them would structured-clone
    // tens of megabytes onto the main thread and throw all of it away — on
    // boot and on tab return, the two frames this plan exists to protect.
    await writeAt(address("ws-1", "file", "src/big.js"), { content: "x".repeat(64) }, NOW - 100 * HOUR);
    await writeAt(address("ws-1", "status"), {}, NOW);
    const watch = await watchBodyReads();

    await lifetime.expireWorkspaceData("dev-1", "ws-1", NOW);
    await lifetime.evictWorkspaceData("dev-1", "ws-1");

    watch.stop();
    expect(watch.read).toEqual([]);
    expect(await held(address("ws-1", "file", "src/big.js"))).toBe(false);
    expect(await held(address("ws-1", "status"))).toBe(false);
  });
});

describe("the recent files", () => {
  it("keeps 5, of at most 1 MB a record, and media of at most 32 MB in pages", () => {
    expect(lifetime.RECENT_FILES).toBe(5);
    expect(lifetime.FILE_MAX_BYTES).toBe(1048576);
    expect(lifetime.FILE_MEDIA_MAX_BYTES).toBe(32 * 1048576);
  });

  it("keeps the five most recently opened and drops the rest", async () => {
    for (let index = 0; index < 7; index += 1) {
      await writeAt(
        address("ws-1", "file", `src/${index}.js`),
        { content: String(index), openedAt: NOW + index },
        NOW,
      );
    }
    await lifetime.trimRecentFiles("dev-1", "ws-1");
    const kept = await cache.cachedSubKeys("dev-1", "ws-1", "file");
    expect(kept.sort()).toEqual(["src/2.js", "src/3.js", "src/4.js", "src/5.js", "src/6.js"]);
  });

  it("reads a file with no opened-at as opened when it was written", async () => {
    await writeAt(address("ws-1", "file", "old.js"), { content: "old" }, NOW - HOUR);
    for (let index = 0; index < 5; index += 1) {
      await writeAt(address("ws-1", "file", `new-${index}.js`), { content: "n" }, NOW);
    }
    await lifetime.trimRecentFiles("dev-1", "ws-1");
    expect(await held(address("ws-1", "file", "old.js"))).toBe(false);
    expect(await held(address("ws-1", "file", "new-0.js"))).toBe(true);
  });

  it("lets go of the pages of every body it drops", async () => {
    for (let index = 0; index < 6; index += 1) {
      await writeAt(address("ws-1", "file", `src/${index}.js`), { file: { paged: true, of: "v1" }, openedAt: NOW + index }, NOW);
      await pages.writeBodyPage(address("ws-1", "file", `src/${index}.js`), { of: "v1", offset: 0, end: 1, total: 2, body: "eA==" });
    }
    await lifetime.trimRecentFiles("dev-1", "ws-1");
    expect((await pages.readBodyPages(address("ws-1", "file", "src/0.js"), "v1")).pages).toHaveLength(0);
    expect((await pages.readBodyPages(address("ws-1", "file", "src/1.js"), "v1")).pages).toHaveLength(1);
  });

  it("leaves five or fewer alone, and touches no other kind", async () => {
    await writeAt(address("ws-1", "file", "a.js"), { content: "a" }, NOW);
    await writeAt(address("ws-1", "tree", "src"), { entries: [] }, NOW);
    expect(await lifetime.trimRecentFiles("dev-1", "ws-1")).toEqual([]);
    expect(await held(address("ws-1", "file", "a.js"))).toBe(true);
    expect(await held(address("ws-1", "tree", "src"))).toBe(true);
  });
});

// The two caps the Thresholds table names on the SPA side are the writer's to
// apply, and nothing applied them: a file body is written straight through to
// a record, so one 40 MB source file under a 1 MB row was the whole of the
// difference between the rule and the code.
describe("putting a file body in the cache", () => {
  const bodyOf = async (path) => (await cache.readCached(address("ws-1", "file", path)))?.value;

  /** An `fs.read` answer as the wire carries one. */
  const read = (over = {}) => ({
    path: "src/a.js",
    size: 5,
    truncated: false,
    mime: "text/plain",
    content_b64: "aGVsbG8=",
    revision: "r-1",
    ...over,
  });

  const keep = (path, file, openedAt = NOW) =>
    lifetime.cacheFileBody({ deviceId: "dev-1", entityId: "ws-1", path, file, openedAt });

  it("stores the read answer whole, and when it was opened", async () => {
    expect(await keep("src/a.js", read())).toBe(true);
    expect(await bodyOf("src/a.js")).toEqual({ file: read(), openedAt: NOW });
  });

  const fileHead = (path) => address("ws-1", "file", path);

  /** A bridge that pages: `bytes` of `whole` from `offset`, cut after a line
   *  end the way the bridge cuts one, all of version `version`. */
  const pager = (whole, { version = "v1" } = {}) => {
    const bytes = Buffer.from(whole, "utf8");
    return vi.fn(async (offset, size = pages.BODY_PAGE_BYTES) => {
      let end = Math.min(bytes.length, offset + size);
      if (end < bytes.length) {
        const newline = bytes.lastIndexOf(10, end - 1);
        if (newline >= offset) end = newline + 1;
      }
      return { of: version, offset, end, total: bytes.length, body: bytes.subarray(offset, end).toString("base64") };
    });
  };

  const bigText = (lines) => Array.from({ length: lines }, (_, index) => `line ${index + 1} ${"x".repeat(60)}`).join("\n");

  // A body over the cap is no longer turned away (#95): the record says what
  // the file is, and its bytes are pages beside it, the first read by range.
  it("keeps a body over 1 MB as a head and its first page, read by range", async () => {
    const whole = bigText(20_000);
    const size = Buffer.byteLength(whole);
    const readPage = pager(whole);
    const answer = read({ path: "big.js", size, truncated: true, editable: false, content_b64: "cut", revision: null });
    expect(await lifetime.cacheFileBody({ deviceId: "dev-1", entityId: "ws-1", path: "big.js", file: answer, openedAt: NOW, readPage }))
      .toBe(true);

    expect(readPage).toHaveBeenCalledTimes(1);
    expect(readPage).toHaveBeenCalledWith(0, pages.BODY_PAGE_BYTES);
    const record = await bodyOf("big.js");
    expect(record.openedAt).toBe(NOW);
    expect(record.file).toEqual({
      path: "big.js", size, mime: "text/plain", truncated: false, editable: false, paged: true, of: "v1",
    });
    const held = await pages.readBodyPages(fileHead("big.js"), "v1");
    expect(held.pages).toHaveLength(1);
    expect(held.total).toBe(size);
    expect(held.complete).toBe(false);
  });

  // An older bridge refuses `range`, so its answer — cut at its wire cap — is
  // what there is. Kept, split into pages, and named incomplete by the file's
  // own size, so nothing opens it as the whole file.
  it("keeps what a bridge that cannot page carried, as pages that do not reach the file's size", async () => {
    const carried = Buffer.from("the first megabyte\n").toString("base64");
    const answer = read({ path: "part.js", size: 2 * lifetime.FILE_MAX_BYTES, truncated: true, content_b64: carried });
    expect(await keep("part.js", answer)).toBe(true);
    const record = await bodyOf("part.js");
    expect(record.file).toMatchObject({ paged: true, of: lifetime.WHOLE_READ, truncated: true, editable: false });
    expect(record.file.content_b64).toBeUndefined();
    const held = await pages.readBodyPages(fileHead("part.js"), lifetime.WHOLE_READ);
    expect(pages.joinedBase64(held.pages)).toBe(carried);
    expect(held.total).toBe(2 * lifetime.FILE_MAX_BYTES);
    expect(held.complete).toBe(false);
  });

  // Every whole read is of the same "version", so a shorter body split over a
  // longer one would chain on into the longer one's last pages.
  it("lets go of the last whole read's pages before splitting the next", async () => {
    const longer = Buffer.from("a".repeat(7 * pages.BODY_PAGE_BYTES)).toString("base64");
    const shorter = Buffer.from("b".repeat(5 * pages.BODY_PAGE_BYTES)).toString("base64");
    await keep("clip.mp3", read({ path: "clip.mp3", mime: "audio/mpeg", size: 7 * pages.BODY_PAGE_BYTES, content_b64: longer }));
    await keep("clip.mp3", read({ path: "clip.mp3", mime: "audio/mpeg", size: 5 * pages.BODY_PAGE_BYTES, content_b64: shorter }));
    const held = await pages.readBodyPages(fileHead("clip.mp3"), lifetime.WHOLE_READ);
    expect(pages.joinedBase64(held.pages)).toBe(shorter);
  });

  it("measures an answer with no size of its own in bytes, not in characters", async () => {
    // Three bytes each: a body of a third of the cap in characters is over it.
    const content = Buffer.from("な".repeat(Math.ceil(lifetime.FILE_MAX_BYTES / 3) + 1)).toString("base64");
    expect(await keep("jp.txt", read({ size: null, content_b64: content }))).toBe(true);
    expect((await bodyOf("jp.txt")).file.paged).toBe(true);
  });

  it("lets go of the pages when a body that fits is written over a paged one", async () => {
    const whole = bigText(20_000);
    await lifetime.cacheFileBody({
      deviceId: "dev-1", entityId: "ws-1", path: "big.js", openedAt: NOW, readPage: pager(whole),
      file: read({ path: "big.js", size: Buffer.byteLength(whole), truncated: true }),
    });
    expect(await keep("big.js", read({ path: "big.js" }))).toBe(true);
    expect((await bodyOf("big.js")).file).toEqual(read({ path: "big.js" }));
    expect(await cache.cachedSubKeys("dev-1", "ws-1", pages.PAGE_RECORD_KIND)).toEqual([]);
  });

  // The viewer shows an image or a video from every one of its bytes, so a
  // part of one is worth nothing: all of it is read into pages, up to the
  // bridge's own media cap.
  it("reads every page of a media file over the cap, a megabyte at a time", async () => {
    const png = "p".repeat(3 * lifetime.FILE_MAX_BYTES + 10);
    const readPage = pager(png);
    const answer = read({ path: "shot.png", mime: "image/png", size: png.length, content_b64: "whole" });
    expect(await lifetime.cacheFileBody({ deviceId: "dev-1", entityId: "ws-1", path: "shot.png", file: answer, openedAt: NOW, readPage }))
      .toBe(true);
    expect(readPage.mock.calls.map(([offset, bytes]) => [offset, bytes])).toEqual([0, 1, 2, 3].map((index) =>
      [index * lifetime.FILE_MAX_BYTES, lifetime.FILE_MAX_BYTES]));
    const held = await pages.readBodyPages(fileHead("shot.png"), "v1");
    expect(held.complete).toBe(true);
    expect(pages.joinedBase64(held.pages)).toBe(Buffer.from(png).toString("base64"));
    expect((await bodyOf("shot.png")).file).toMatchObject({ paged: true, of: "v1", mime: "image/png" });
  });

  it("keeps only what a media file past the media cap is, and reads none of it", async () => {
    const readPage = pager("x");
    const answer = read({ path: "film.mp4", mime: "video/mp4", size: lifetime.FILE_MEDIA_MAX_BYTES + 1, truncated: true, content_b64: "" });
    expect(await lifetime.cacheFileBody({ deviceId: "dev-1", entityId: "ws-1", path: "film.mp4", file: answer, openedAt: NOW, readPage }))
      .toBe(true);
    expect(readPage).not.toHaveBeenCalled();
    const record = await bodyOf("film.mp4");
    expect(record.file).toMatchObject({ truncated: true, size: lifetime.FILE_MEDIA_MAX_BYTES + 1 });
    expect(record.file.paged).toBeUndefined();
    expect(await cache.cachedSubKeys("dev-1", "ws-1", pages.PAGE_RECORD_KIND)).toEqual([]);
  });

  it("keeps only the size of a binary file over the cap: the viewer shows nothing else", async () => {
    const readPage = pager("x");
    const answer = read({ path: "blob.bin", mime: "application/octet-stream", size: 5 * lifetime.FILE_MAX_BYTES, truncated: true });
    await lifetime.cacheFileBody({ deviceId: "dev-1", entityId: "ws-1", path: "blob.bin", file: answer, openedAt: NOW, readPage });
    expect(readPage).not.toHaveBeenCalled();
    expect((await bodyOf("blob.bin")).file.content_b64).toBeUndefined();
  });

  it("re-stamps a paged head handed back with no reader, and reads nothing", async () => {
    const whole = bigText(20_000);
    await lifetime.cacheFileBody({
      deviceId: "dev-1", entityId: "ws-1", path: "big.js", openedAt: NOW, readPage: pager(whole),
      file: read({ path: "big.js", size: Buffer.byteLength(whole), truncated: true }),
    });
    const head = (await bodyOf("big.js")).file;
    expect(await keep("big.js", head, NOW + 1)).toBe(true);
    expect(await bodyOf("big.js")).toEqual({ file: head, openedAt: NOW + 1 });
    expect((await pages.readBodyPages(fileHead("big.js"), "v1")).pages).toHaveLength(1);
  });

  // A refresh reads the first page again. The same version keeps every page
  // read since; another version starts the body over.
  it("refreshes a paged head from its first page, keeping the pages of the same version", async () => {
    const whole = bigText(20_000);
    const first = pager(whole);
    const file = read({ path: "big.js", size: Buffer.byteLength(whole), truncated: true });
    await lifetime.cacheFileBody({ deviceId: "dev-1", entityId: "ws-1", path: "big.js", file, openedAt: NOW, readPage: first });
    const firstPage = await first.mock.results[0].value;
    await pages.writeBodyPage(fileHead("big.js"), await first(firstPage.end));
    const head = (await bodyOf("big.js")).file;

    await lifetime.cacheFileBody({ deviceId: "dev-1", entityId: "ws-1", path: "big.js", file: head, openedAt: NOW, readPage: pager(whole) });
    expect((await pages.readBodyPages(fileHead("big.js"), "v1")).pages).toHaveLength(2);

    const changed = pager(`${whole}\nmore`, { version: "v2" });
    await lifetime.cacheFileBody({ deviceId: "dev-1", entityId: "ws-1", path: "big.js", file: head, openedAt: NOW, readPage: changed });
    expect(changed).toHaveBeenCalledWith(0, pages.BODY_PAGE_BYTES);
    expect((await bodyOf("big.js")).file.of).toBe("v2");
    expect((await pages.readBodyPages(fileHead("big.js"), "v1")).pages).toHaveLength(0);
    expect((await pages.readBodyPages(fileHead("big.js"), "v2")).pages).toHaveLength(1);
  });

  it("leaves at most five bodies behind it", async () => {
    for (let index = 0; index < 7; index += 1) {
      await keep(`src/${index}.js`, read({ path: `src/${index}.js` }), NOW + index);
    }
    expect((await cache.cachedSubKeys("dev-1", "ws-1", "file")).sort())
      .toEqual(["src/2.js", "src/3.js", "src/4.js", "src/5.js", "src/6.js"]);
  });
});

describe("measuring a body against a cap", () => {
  it("counts bytes, and answers the empty and the absent as within", () => {
    expect(lifetime.withinBytes("abc", 3)).toBe(true);
    expect(lifetime.withinBytes("abcd", 3)).toBe(false);
    expect(lifetime.withinBytes("なな", 5)).toBe(false); // six bytes, not two characters
    expect(lifetime.withinBytes("", 0)).toBe(true);
    expect(lifetime.withinBytes(null, 0)).toBe(true);
  });
});
