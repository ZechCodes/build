// @vitest-environment jsdom
// The Changes surface over bodies too large for one record (#95): a commit's
// patch and an uncommitted file's diff are kept in pages, painted from the
// cache, and read on a page at a time — only from a bridge that announced
// `bodies.pages`, and only as the reader reaches the end of what is drawn.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

import { pagedAnswer, patchFor, worktreeOf } from "./gitWireFixture.js";

/** Reading a body through to its end is many page reads, each through the
 *  cache and a paint; on a loaded machine that outlasts waitFor's one second. */
const READ_THROUGH = { timeout: 15000 };

const HASH = "a".repeat(40);
const tree = worktreeOf({ "src/a.js": "new line" });

const log = () => ({
  branch: "main",
  commits: [{ hash: HASH, short: "aaaaaaa", subject: "earlier work", author: "Ada", email: "z@x", time: 1 }],
  more: false,
});

const show = (overrides = {}) => ({
  hash: HASH,
  short: "aaaaaaa",
  subject: "earlier work",
  body: "why it happened",
  author: "Ada",
  email: "z@x",
  stat: { files_changed: 2, insertions: 2, deletions: 2 },
  patch: patchFor("src/b.js", "committed line"),
  truncated: false,
  ...overrides,
});

// A commit patch well over the 256 KB a single record holds: many lines of one
// file, then a second file.
const bigLines = Array.from({ length: 30_000 }, (_unused, index) => `line ${index} of the big file`);
const BIG_PATCH = `${patchFor("src/big.js", bigLines)}${patchFor("src/tail.js", "the last file")}`;

let mountGitPane, cache, pages, events, scopeOf;

const settle = async () => {
  for (let i = 0; i < 30; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = "";
  ({ scopeFor: scopeOf } = await import("../src/core/cacheScope.js"));
  cache = await import("../src/core/localCache.js");
  pages = await import("../src/core/bodyPages.js");
  events = await import("../src/core/changeEvents.js");
  ({ mountGitPane } = await import("../src/core/gitPane.js"));
});

afterEach(() => events.resetChangeEvents());

/** The machine's bridge, greeted as one that can cut pages. */
const greetPaging = () =>
  events.greetBridge(async () => ({ api_version: "2.0.0", capabilities: ["bodies.pages", "diffs.perFile"] }), {
    deviceId: "dev-1",
  });

const patchAddress = { deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: HASH };

const seedShape = async () => {
  await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, tree.status());
  await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, log());
};

const mountPane = async (callRpc) => {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const pane = mountGitPane(container, { scope: { run_id: "run-1" }, callRpc, cacheScope: scopeOf("dev-1") });
  await settle();
  return { container, pane };
};

/** The bytes of `text`, as the bridge counts a range. */
const bytes = (text) => new TextEncoder().encode(text).length;

/** The digest a bridge names `BIG_PATCH`'s pages by. */
const PATCH_VERSION = "p1";

/** A bridge answering the commit `whole` cut at `cutAt` characters, and a
 *  ranged `git.show` with the next stretch of it, named `PATCH_VERSION`. */
const commitRpc = (whole, { cutAt = null } = {}) =>
  vi.fn(async (method, params) => {
    if (method === "git.status") return tree.status();
    if (method === "git.diff") return tree.diff(params);
    if (method === "git.log") return log();
    if (method !== "git.show") return {};
    if (!params.range) {
      return cutAt === null
        ? show({ patch: whole })
        : show({ patch: whole.slice(0, cutAt), truncated: true, patch_bytes: bytes(whole) });
    }
    const encoded = new TextEncoder().encode(whole);
    const start = params.range.offset;
    let end = Math.min(encoded.length, start + params.range.bytes);
    if (end < encoded.length) end = encoded.lastIndexOf(10, end - 1) + 1;
    return {
      ...show({ patch: new TextDecoder().decode(encoded.subarray(start, end)) }),
      range: { offset: start, end, total: encoded.length, version: PATCH_VERSION },
    };
  });

const openCommit = async (container) => {
  container.querySelector(".crow").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
  await settle();
};

/** The reader opens the first file of the commit — the big one its pages end
 *  inside — out of its capped preview. */
const openBigFile = async (container) => {
  container.querySelector(".file .dscroll").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
  await settle();
};

const showCalls = (callRpc) => callRpc.mock.calls.filter(([method]) => method === "git.show").map(([, params]) => params);

describe("a commit patch too big for one record", () => {
  it("is kept as a head and pages, painted from them, and read back by the next mount with no wire call", async () => {
    await seedShape();
    const callRpc = commitRpc(BIG_PATCH);
    const first = await mountPane(callRpc);
    await openCommit(first.container);

    const head = (await cache.readCached(patchAddress)).value;
    expect(head).toMatchObject({ hash: HASH, paged: true, subject: "earlier work" });
    expect(head.patch).toBeUndefined();
    const held = await pages.readBodyPages(patchAddress, "whole");
    expect(held.pages.length).toBeGreaterThan(1);
    expect(held.complete).toBe(true);
    expect(first.container.textContent).toContain("why it happened");
    expect(first.container.textContent).toContain("src/tail.js");
    expect(first.container.textContent).not.toContain("diff truncated");
    first.pane.dispose();

    const again = commitRpc(BIG_PATCH);
    const second = await mountPane(again);
    await openCommit(second.container);
    expect(showCalls(again)).toEqual([]);
    expect(second.container.textContent).toContain("src/tail.js");
    second.pane.dispose();
  });

  it("reads the rest of a cut patch a page at a time as the reader reads down the file it ends in", async () => {
    await greetPaging();
    await seedShape();
    const cutAt = BIG_PATCH.indexOf("line 2000 ");
    const callRpc = commitRpc(BIG_PATCH, { cutAt });
    const { container, pane } = await mountPane(callRpc);
    await openCommit(container);
    await openBigFile(container);

    // jsdom lays nothing out, so the open file's end is always in reach:
    // every paint reads on until the pages meet the whole.
    await vi.waitFor(async () => expect((await pages.readBodyPages(patchAddress, PATCH_VERSION)).complete).toBe(true), READ_THROUGH);
    await settle();
    const ranged = showCalls(callRpc).filter((params) => params.range);
    expect(ranged.length).toBeGreaterThan(1);
    // The pages are the bridge's own from the first, named by its digest of
    // the patch — not the cut answer's, which names no version.
    expect(ranged[0]).toEqual({ run_id: "run-1", hash: HASH, range: { offset: 0, bytes: pages.BODY_PAGE_BYTES } });
    expect((await cache.readCached(patchAddress)).value.of).toBe(PATCH_VERSION);
    expect(ranged.every((params) => params.max_bytes === undefined)).toBe(true);
    expect(container.textContent).toContain("src/tail.js");
    expect(container.querySelector("[data-more-key]")).toBeNull();
    expect(container.textContent).not.toContain("diff truncated");
    pane.dispose();
  });

  // What the commit says comes from its pages alone: painted before the
  // bridge greets, it shows how much it holds and is marked to read on, and a
  // bridge that cannot page is simply never sent a range. Once the bridge has
  // greeted, the reader reaching the end reads on — no remount needed.
  it("says how much a cut patch holds before its bridge greets, and reads on once it has", async () => {
    await seedShape();
    const cutAt = BIG_PATCH.indexOf("line 2000 ");
    const callRpc = commitRpc(BIG_PATCH, { cutAt });
    const { container, pane } = await mountPane(callRpc);
    await openCommit(container);

    expect(showCalls(callRpc)).toEqual([{ run_id: "run-1", hash: HASH }]);
    expect((await cache.readCached(patchAddress)).value).toMatchObject({ paged: true, truncated: true, of: "whole" });
    const held = await pages.readBodyPages(patchAddress, "whole");
    expect(pages.joinedText(held.pages)).toBe(BIG_PATCH.slice(0, BIG_PATCH.lastIndexOf("\n", cutAt) + 1));
    expect(held).toMatchObject({ total: bytes(BIG_PATCH), complete: false });
    expect(container.textContent).not.toContain("diff truncated");
    expect(container.querySelector('[data-more-key="commit"]')?.textContent).toMatch(/^Showing .* of /);

    // The bridge's pages are of its own digest, not of the answer split here:
    // the commit is read again from the top in them.
    await greetPaging();
    await openBigFile(container);
    container.querySelector(".cdetail-host").dispatchEvent(new window.Event("scroll"));
    await vi.waitFor(async () => expect((await pages.readBodyPages(patchAddress, PATCH_VERSION)).complete).toBe(true), READ_THROUGH);
    await vi.waitFor(() => expect(container.textContent).toContain("src/tail.js"), READ_THROUGH);
    expect(container.querySelector("[data-more-key]")).toBeNull();
    pane.dispose();
  });

  // The same commit renders another patch once `.git/info/attributes` or a
  // diff setting changes: its hash names neither. A page cut from the new
  // patch is never joined to the old one's — the commit is read again from
  // the top, and every page held is of the patch on screen (#95 round 2).
  it("reads a commit again from the top when its patch changes between pages, joining no two patches", async () => {
    await greetPaging();
    await seedShape();
    const OTHER = `${patchFor("src/big.js", bigLines.map((line) => `${line}, rendered again`))}${patchFor("src/tail.js", "the other last file")}`;
    let current = { whole: BIG_PATCH, version: "p1" };
    const cutAt = BIG_PATCH.indexOf("line 2000 ");
    const callRpc = vi.fn(async (method, params) => {
      if (method === "git.status") return tree.status();
      if (method === "git.diff") return tree.diff(params);
      if (method === "git.log") return log();
      if (method !== "git.show") return {};
      const { whole, version } = current;
      if (!params.range) return show({ patch: whole.slice(0, cutAt), truncated: true, patch_bytes: bytes(whole) });
      const answer = { ...show(), ...pagedAnswer(whole, params.range.offset, { version }) };
      // The attributes change right after the first page is cut.
      if (params.range.offset === 0 && version === "p1") current = { whole: OTHER, version: "p2" };
      return answer;
    });
    const { container, pane } = await mountPane(callRpc);
    await openCommit(container);
    await openBigFile(container);

    await vi.waitFor(async () => expect((await pages.readBodyPages(patchAddress, "p2")).complete).toBe(true), READ_THROUGH);
    await settle();
    expect((await cache.readCached(patchAddress)).value.of).toBe("p2");
    expect(pages.joinedText((await pages.readBodyPages(patchAddress, "p2")).pages)).toBe(OTHER);
    expect((await pages.readBodyPages(patchAddress, "p1")).pages).toEqual([]);
    await vi.waitFor(() => expect(container.textContent).toContain("the other last file"), READ_THROUGH);
    expect(container.textContent).not.toContain("the last file");
    expect(showCalls(callRpc).filter((params) => !params.range)).toHaveLength(2);
    pane.dispose();
  });

  // The pages end inside a file drawn capped: the stack is no taller however
  // many pages land in it, so its end stays in reach. Nothing is read on
  // until the reader is shown that file's rows (#95 round 2).
  it("reads no page past a capped file's preview until the reader opens it", async () => {
    await greetPaging();
    await seedShape();
    const cutAt = BIG_PATCH.indexOf("line 2000 ");
    const callRpc = commitRpc(BIG_PATCH, { cutAt });
    const { container, pane } = await mountPane(callRpc);
    await openCommit(container);
    const host = container.querySelector(".cdetail-host");
    for (let scrolls = 0; scrolls < 5; scrolls++) {
      host.dispatchEvent(new window.Event("scroll"));
      await settle();
    }
    const ranged = () => showCalls(callRpc).filter((params) => params.range).map((params) => params.range.offset);
    expect(ranged()).toEqual([0]);
    expect(container.querySelector('[data-more-key="commit"]')?.dataset.moreFile).toBe(container.querySelector(".file").dataset.key);

    await openBigFile(container);
    await vi.waitFor(() => expect(ranged().length).toBeGreaterThan(1), READ_THROUGH);
    pane.dispose();
  });

  // A lockfile's pages end inside a file folded into the collapsed
  // generated-files group: with none of its rows drawn, nothing reads on
  // until the reader opens the group and reads down to the file's end.
  it("reads no page past a file in the collapsed generated-files group until the reader opens it (#95 round 3)", async () => {
    await greetPaging();
    await seedShape();
    const lockPatch = `${patchFor("package-lock.json", bigLines)}${patchFor("src/tail.js", "the last file")}`;
    const callRpc = commitRpc(lockPatch, { cutAt: lockPatch.indexOf("line 2000 ") });
    const { container, pane } = await mountPane(callRpc);
    await openCommit(container);
    const host = container.querySelector(".cdetail-host");
    for (let scrolls = 0; scrolls < 5; scrolls++) {
      host.dispatchEvent(new window.Event("scroll"));
      await settle();
    }
    const ranged = () => showCalls(callRpc).filter((params) => params.range).map((params) => params.range.offset);
    expect(container.querySelector(".noisehead")).not.toBeNull();
    expect(container.querySelector('.file[data-key*="package-lock.json"]')).toBeNull();
    expect(ranged()).toEqual([0]);

    container.querySelector(".noisehead").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    host.dispatchEvent(new window.Event("scroll"));
    await settle();
    // Opened, the lockfile is drawn capped: still nothing past its preview.
    expect(ranged()).toEqual([0]);

    container.querySelector('.file[data-key*="package-lock.json"] .dscroll').dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    await vi.waitFor(() => expect(ranged().length).toBeGreaterThan(1), READ_THROUGH);
    pane.dispose();
  });

  // Two heads written on one millisecond share `at`: the pages joined under
  // the first are not read on from under the second (#95 round 4).
  it("joins no page held under one head onto the pages of a head written on the same millisecond", async () => {
    const now = vi.spyOn(Date, "now").mockReturnValue(1_789_000_000_000);
    try {
      await seedShape();
      const oldPart = patchFor("src/old.js", "old line");
      const newPart = patchFor("src/new.js", "new line");
      const endPart = patchFor("src/end.js", "end line");
      expect(bytes(oldPart)).toBe(bytes(newPart));
      const cut = bytes(newPart);
      const total = cut + bytes(endPart);
      const head = (of) => {
        const value = { ...show(), paged: true, of };
        delete value.patch;
        return value;
      };
      await pages.writeBodyPage(patchAddress, { of: "v1", offset: 0, end: cut, total, body: oldPart });
      await cache.writeCached(patchAddress, head("v1"));
      const first = await cache.readCached(patchAddress);
      // A bridge that never answers: what is painted is the cache's alone.
      const callRpc = vi.fn(async (method, params) => {
        if (method === "git.status") return tree.status();
        if (method === "git.diff") return tree.diff(params);
        if (method === "git.log") return log();
        return new Promise(() => {});
      });
      const { container, pane } = await mountPane(callRpc);
      await openCommit(container);
      await vi.waitFor(() => expect(container.textContent).toContain("src/old.js"));

      await pages.writeBodyPages(patchAddress, [
        { of: "v2", offset: 0, end: cut, total, body: newPart },
        { of: "v2", offset: cut, end: total, total, body: endPart },
      ]);
      await cache.writeCached(patchAddress, head("v2"));
      expect((await cache.readCached(patchAddress)).at).toBe(first.at);

      await vi.waitFor(() => expect(container.textContent).toContain("src/end.js"));
      expect(container.textContent).toContain("src/new.js");
      expect(container.textContent).not.toContain("src/old.js");
      pane.dispose();
    } finally {
      now.mockRestore();
    }
  });

  // A page that lands after the commit's head was let go of — a pass dropped
  // the commit once it was published — is not kept without a head, and the
  // pane stops holding the commit it no longer has a record of.
  it("keeps no page of a head let go of while it was read, and forgets the commit", async () => {
    await greetPaging();
    await seedShape();
    const cutAt = BIG_PATCH.indexOf("line 2000 ");
    const cutShow = show({ patch: BIG_PATCH.slice(0, cutAt), truncated: true, patch_bytes: bytes(BIG_PATCH) });
    let answerPage;
    let unranged = 0;
    const callRpc = vi.fn(async (method, params) => {
      if (method === "git.status") return tree.status();
      if (method === "git.diff") return tree.diff(params);
      if (method === "git.log") return log();
      if (method !== "git.show") return {};
      const page = () => ({ ...show(), ...pagedAnswer(BIG_PATCH, params.range.offset, { version: PATCH_VERSION }) });
      if (params.range?.offset === 0) return page();
      if (params.range) return new Promise((resolve) => { answerPage = () => resolve(page()); });
      unranged += 1;
      return unranged === 1 ? cutShow : show();
    });
    const { container, pane } = await mountPane(callRpc);
    await openCommit(container);
    await openBigFile(container);
    await vi.waitFor(() => expect(answerPage).toBeTypeOf("function"));

    await pages.dropBodyPages(patchAddress);
    await cache.deleteCached([patchAddress]);
    await settle();
    answerPage();
    await settle();

    expect(await cache.cachedSubKeys("dev-1", "run-1", "page")).toEqual([]);
    expect(showCalls(callRpc).filter((params) => params.range)).toHaveLength(2);
    // The commit on screen lost its record: it is read again, whole this time.
    await vi.waitFor(() => expect(container.textContent).toContain("committed line"));
    expect((await cache.readCached(patchAddress)).value.paged).toBeUndefined();
    pane.dispose();
  });
});

describe("an uncommitted file cut by the bridge", () => {
  const [statusFileA] = tree.status().files;
  const extra = Array.from({ length: 12 }, (_unused, index) => `+added line ${index}`).join("\n");
  const WHOLE = `${tree.diff({ paths: ["src/a.js"] }).files[0].patch}${extra}\n+the very last line\n`;
  /** A bridge that pages `git.diff` in 60-byte pages named "v-a", and answers
   *  the file unranged cut short. */
  const cutDiff = (params) => {
    const file = { path: "src/a.js", content_key: statusFileA.content_key };
    if (params.range) {
      const page = pagedAnswer(WHOLE, params.range.offset, { version: "v-a", pageBytes: 60 });
      return { files: [{ ...file, patch: page.patch, range: page.range }] };
    }
    return { files: [{ ...file, patch: WHOLE.slice(0, 150), truncated: true }] };
  };
  const diffRpc = () =>
    vi.fn(async (method, params) => (method === "git.diff" ? cutDiff(params) : method === "git.status" ? tree.status() : method === "git.log" ? log() : {}));
  const rangedOffsets = (callRpc) =>
    callRpc.mock.calls.filter(([method, params]) => method === "git.diff" && params.range).map(([, params]) => params.range.offset);

  const openFileBody = async (container) => {
    container.querySelector(".file .dscroll").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
  };

  it("keeps the bridge's own pages and reads on once the reader opens the file", async () => {
    await greetPaging();
    await seedShape();
    const callRpc = diffRpc();
    const { container, pane } = await mountPane(callRpc);
    expect(container.querySelector('.fmore[data-more-key="EDIT:src/a.js"]')?.textContent).toMatch(/^Showing .* of /);
    expect(rangedOffsets(callRpc)).toEqual([0]);

    await openFileBody(container);
    await vi.waitFor(() => expect(container.textContent).toContain("the very last line"), READ_THROUGH);
    expect(rangedOffsets(callRpc).length).toBeGreaterThan(2);
    expect(new Set(rangedOffsets(callRpc)).size).toBe(rangedOffsets(callRpc).length);
    expect(container.querySelector(".fmore")).toBeNull();
    pane.dispose();
  });

  // Painted before the bridge greets, a cut file says so and is still marked
  // to read on: nothing asks for a range until the bridge can cut one, and
  // then the reader reaching the end reads on — no remount needed.
  it("says a file was cut before its bridge greets, and reads on once it has", async () => {
    await seedShape();
    const callRpc = diffRpc();
    const { container, pane } = await mountPane(callRpc);
    await openFileBody(container);
    expect(container.querySelector('.ftrunc[data-more-key="EDIT:src/a.js"]')?.textContent).toBe("diff truncated at 1 MiB");
    expect(rangedOffsets(callRpc)).toEqual([]);

    await greetPaging();
    container.querySelector(".cdetail-host").dispatchEvent(new window.Event("scroll"));
    await vi.waitFor(() => expect(container.textContent).toContain("the very last line"), READ_THROUGH);
    expect(container.querySelector("[data-more-key]")).toBeNull();
    pane.dispose();
  });
});
