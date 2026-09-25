// `issues.list` a page at a time (#85): each page lands under its own address,
// is read back from there, and is laid over the held list for the stretch of
// numbers it answers — never over a row something newer wrote.
import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const issue = (number, over = {}) => ({
  id: `issue-${number}`,
  number,
  title: `issue ${number}`,
  updated_at: "2026-09-25T08:00:00Z",
  ...over,
});
const numbers = (issues) => issues.map((one) => one.number);
const AT = (text) => Date.parse(text);

let pages, cache, trackerCache;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  trackerCache = await import("../src/core/trackerCache.js");
  pages = await import("../src/core/trackerPages.js");
});

describe("laying a page over a held list", () => {
  const stretch = (issues, over = {}) => ({ issues, above: Infinity, through: -Infinity, readAt: null, ...over });

  it("is the page itself over nothing", () => {
    expect(numbers(pages.withIssuePage(null, stretch([issue(3), issue(2)])))).toEqual([3, 2]);
  });

  it("replaces only the numbers the page answers for", () => {
    const held = [issue(9), issue(8), issue(7), issue(6), issue(5), issue(4)];
    const laid = pages.withIssuePage(held, stretch([issue(7, { title: "new" }), issue(5)], { above: 8, through: 5 }));
    expect(numbers(laid)).toEqual([9, 8, 7, 5, 4]);
    expect(laid.find((one) => one.number === 7).title).toBe("new");
  });

  it("lets the last page answer for every number under the one before it", () => {
    const held = [issue(4), issue(3), issue(2), issue(1)];
    expect(numbers(pages.withIssuePage(held, stretch([issue(3)], { above: 4 })))).toEqual([4, 3]);
  });

  it("keeps a held row written after the page's copy of it", () => {
    const newer = issue(5, { title: "pushed", updated_at: "2026-09-25T08:05:00Z" });
    const laid = pages.withIssuePage([newer], stretch([issue(5, { title: "paged" })]));
    expect(laid).toEqual([newer]);
  });

  it("takes the page's row over a held copy no newer than it", () => {
    const held = [issue(5, { title: "held" })];
    expect(pages.withIssuePage(held, stretch([issue(5, { title: "paged" })]))[0].title).toBe("paged");
  });

  it("keeps a row the page does not name when it was written after the page was read", () => {
    const filed = issue(6, { updated_at: "2026-09-25T08:10:00Z" });
    const gone = issue(4, { updated_at: "2026-09-25T07:00:00Z" });
    const laid = pages.withIssuePage([filed, gone], stretch([issue(5)], { readAt: AT("2026-09-25T08:09:00Z") }));
    expect(numbers(laid)).toEqual([6, 5]);
  });
});

describe("pulling every page", () => {
  const LIST = Array.from({ length: 7 }, (_, index) => issue(7 - index));
  /** A bridge that pages the way #85's does: number descending, and a cursor
   *  that is the last number a page answered. */
  const pagingBridge = (list = LIST) => vi.fn(async (params) => {
    const below = params.cursor ? Number(params.cursor.slice(1)) : Infinity;
    const rest = list.filter((one) => one.number < below);
    const page = rest.slice(0, params.limit);
    const more = rest.length > params.limit;
    return {
      project_id: "p1",
      issues: page,
      user_session: { gap_ms: 1, now_ms: AT("2026-09-25T08:30:00Z") },
      ...(more ? { next_cursor: `c${page.at(-1).number}` } : {}),
    };
  });

  const pull = (ask, fold, extra = {}) => pages.pullIssuePages({
    ask, deviceId: "dev-1", projectId: "p1", params: { project_id: "p1" }, fold, limit: 3, ...extra,
  });

  it("asks page after page with the cursor each answered, and folds each in order", async () => {
    const ask = pagingBridge();
    const folded = [];
    expect(await pull(ask, async (stretch) => folded.push(stretch))).toBe(true);
    expect(ask.mock.calls.map(([params]) => params)).toEqual([
      { project_id: "p1", limit: 3 },
      { project_id: "p1", limit: 3, cursor: "c5" },
      { project_id: "p1", limit: 3, cursor: "c2" },
    ]);
    expect(folded.map((stretch) => [numbers(stretch.issues), stretch.above, stretch.through])).toEqual([
      [[7, 6, 5], Infinity, 5],
      [[4, 3, 2], 5, 2],
      [[1], 2, -Infinity],
    ]);
    expect(folded[0].readAt).toBe(AT("2026-09-25T08:30:00Z"));
  });

  it("writes each page under its own address and folds what it reads back from there", async () => {
    const seen = [];
    await pull(pagingBridge(), async (stretch, page) => {
      const address = trackerCache.issuesPageAddress("dev-1", "p1", { project_id: "p1", limit: 3, ...(stretch.above === Infinity ? {} : { cursor: `c${stretch.above}` }) });
      seen.push((await cache.readCached(address))?.value);
      expect(page).toEqual(seen.at(-1));
    });
    expect(seen.map((page) => numbers(page.issues))).toEqual([[7, 6, 5], [4, 3, 2], [1]]);
  });

  it("forgets the pages an earlier pull of the same list left behind, and no other list's", async () => {
    const stale = trackerCache.issuesPageAddress("dev-1", "p1", { project_id: "p1", limit: 3, cursor: "c6" });
    const narrowed = trackerCache.issuesPageAddress("dev-1", "p1", { project_id: "p1", state: "open", limit: 3 });
    await cache.writeCached(stale, { issues: [] });
    await cache.writeCached(narrowed, { issues: [] });
    await pull(pagingBridge(), async () => {});
    expect(await cache.readCached(stale)).toBeUndefined();
    expect(await cache.readCached(narrowed)).toBeTruthy();
    expect(await cache.cachedSubKeys("dev-1", "p1", trackerCache.TRACKER_ISSUES_PAGE_KIND)).toHaveLength(4);
  });

  it("stops where the bridge stops answering, keeping what already landed", async () => {
    const ask = pagingBridge();
    ask.mockImplementationOnce(ask.getMockImplementation()).mockResolvedValueOnce(null);
    const folded = [];
    expect(await pull(ask, async (stretch) => folded.push(stretch))).toBe(false);
    expect(folded).toHaveLength(1);
  });

  it("stops when its caller stands down, without writing the answer that came back", async () => {
    let live = true;
    const ask = pagingBridge();
    const fold = vi.fn(async () => { live = false; });
    expect(await pull(ask, fold, { active: () => live })).toBe(false);
    expect(ask).toHaveBeenCalledTimes(1);
    expect(fold).toHaveBeenCalledTimes(1);
  });

  it("walks on past a short or empty page while the bridge names the next", async () => {
    // A label few issues carry: the bridge reads a bounded stretch per page
    // and answers what it kept there, short or nothing, with where to go on.
    const answers = [
      { issues: [issue(9)], next_cursor: "c6" },
      { issues: [], next_cursor: "c3" },
      { issues: [issue(2)] },
    ];
    const ask = vi.fn(async () => ({ project_id: "p1", ...answers.shift() }));
    const folded = [];
    expect(await pull(ask, async (stretch) => folded.push(stretch))).toBe(true);
    expect(ask.mock.calls.map(([params]) => params.cursor)).toEqual([undefined, "c6", "c3"]);
    // The empty page answers for no number: the next one starts where the
    // last row kept left off, and lays itself over everything below that.
    expect(folded.map((stretch) => [numbers(stretch.issues), stretch.above, stretch.through])).toEqual([
      [[9], Infinity, 9],
      [[2], 9, -Infinity],
    ]);
  });

  it("stops rather than loop on a cursor that does not move", async () => {
    const ask = vi.fn(async () => ({ issues: [issue(3)], next_cursor: "same" }));
    await pull(ask, async () => {});
    expect(ask).toHaveBeenCalledTimes(2);
  });
});
