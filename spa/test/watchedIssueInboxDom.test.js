// @vitest-environment jsdom
// #125: a watched issue's inbox row paints from the cache on a reload, before
// the bridge answers anything, and Stop watching takes it away at once and
// puts it back if the bridge refuses. Real cache, feed, device registry and
// rail; the session's `call` is the only stand-in.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { issue, issueDetail } from "./trackerWireFixture.js";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
const DEVICE = "watch-device";
const PROJECT = "proj-1";
const WAIT = { timeout: 5000, interval: 20 };
const GREETING = { api_version: "1.21.0", push_events: true, issues: { watching: true, attachments: true } };
const project = { id: PROJECT, project_id: PROJECT, name: "Build", deviceId: DEVICE, projectKey: `${DEVICE}|${PROJECT}` };
const review = issue({ id: "issue-7", number: 7, title: "Wire 1.22", watched: true, status: "in_review", updated_at: "2026-09-24T01:00:00Z" });

let modules, answers;
const rows = () => [...document.querySelectorAll("#inbox-list .inbox-entry")];
const rowFor = (id) => rows().find((row) => row.dataset.key === `tracker_issue:${id}`) || null;

/** The bridge: greets, and answers what a case scripted; anything else never
 *  answers, so what paints came from the cache. */
const call = vi.fn((method, params) => {
  if (method === "session.hello") return Promise.resolve(GREETING);
  const answer = answers[method];
  return answer ? answer(params) : new Promise(() => {});
});

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = bodyHtml;
  answers = {};
  call.mockClear();
  const { App } = await import("../src/app.js");
  Object.assign(App, { route: { name: "inbox" }, devices: [{ id: DEVICE, name: "Laptop", status: "online" }],
    selectedDeviceId: DEVICE, deviceFilter: null });
  modules = {
    cache: await import("../src/core/localCache.js"),
    tracker: await import("../src/core/trackerCache.js"),
    taskFeed: await import("../src/core/taskFeed.js"),
    inboxView: await import("../src/core/inboxView.js"),
    deviceContexts: await import("../src/core/deviceContexts.js"),
    connection: await import("../src/connection.js"),
  };
  // A reload: the feed and the issue records are already on disk.
  const address = (kind) => ({ deviceId: DEVICE, entityId: "", kind });
  await modules.cache.writeCached(address("feed"), { items: [], runs: [], projects: [project], workspaces: [] });
  await modules.cache.writeCached(address("projects"), [project]);
  await modules.cache.writeCached(address("workspaces"), []);
  await modules.tracker.writeIssuesRecord(DEVICE, PROJECT, modules.tracker.issuesRecord([review], []));
  await modules.tracker.writeIssueRecord(DEVICE, PROJECT, review.id, issueDetail(review, []));
  const context = modules.deviceContexts.adoptDeviceSession({
    deviceId: DEVICE, call, close: () => {}, peer: () => {}, onCarrier: () => {},
    installAdapter: (selection) => selection.create(call),
  });
  await modules.connection.greetLiveBridge(context);
  modules.inboxView.setInboxView("inbox");
  modules.inboxView.mountInboxList();
  await modules.taskFeed.startFeed();
});

afterEach(() => {
  modules.inboxView.unmountInboxList();
  modules.taskFeed.stopFeed();
  modules.deviceContexts.resetDeviceContexts();
});

const unwatch = async () => {
  rowFor(review.id).querySelector("[data-menu]").click();
  await vi.waitFor(() => expect(rowFor(review.id).querySelector("[data-unwatch]")).not.toBe(null), WAIT);
  rowFor(review.id).querySelector("[data-unwatch]").click();
};

describe("a watched issue's inbox row", () => {
  it("paints from the cache on a reload, on both faces, and opens the issue", async () => {
    await vi.waitFor(() => expect(rowFor(review.id)?.querySelector(".inbox-facts")?.textContent).toBe("In review"), WAIT);
    expect(call.mock.calls.some(([method]) => method === "issues.list" || method === "issues.get")).toBe(false);
    modules.inboxView.setInboxView("projects");
    await vi.waitFor(() => expect(rowFor(review.id)).not.toBe(null), WAIT);
    expect(rowFor(review.id).closest(".inbox-project")?.dataset.project).toBe(project.projectKey);
    modules.inboxView.setInboxView("inbox");
  });

  it("goes the moment Stop watching is pressed, and the cached list says so", async () => {
    answers["issues.unwatch"] = async () => ({ issue: { ...review, watched: false } });
    await vi.waitFor(() => expect(rowFor(review.id)).not.toBe(null), WAIT);
    await unwatch();
    await vi.waitFor(() => expect(rowFor(review.id)).toBe(null), WAIT);
    expect(call).toHaveBeenCalledWith("issues.unwatch", { issue_id: review.id });
    const held = await modules.tracker.readIssuesRecord(DEVICE, PROJECT);
    expect(held.issues[0].watched).toBe(false);
  });

  it("stays gone when a list asked before the unwatch landed arrives after the click", async () => {
    let answer;
    answers["issues.unwatch"] = () => new Promise((resolve) => { answer = resolve; });
    // A second watched issue in review is how each list is seen to have been
    // painted: its row, and its title, come from that list and nothing else.
    const other = issue({ id: "issue-8", number: 8, title: "Marker", watched: true, status: "in_review",
      updated_at: "2026-09-24T01:01:00Z" });
    // What the sync layer writes when an `issues.list` answer lands
    // (core/cacheSync.js readIssues): the whole list, over whatever was held.
    const land = (issues) => modules.tracker.writeIssuesRecord(DEVICE, PROJECT, modules.tracker.issuesRecord(issues, []));
    const titleOf = (id) => rowFor(id)?.querySelector(".stitle")?.textContent;
    await vi.waitFor(() => expect(rowFor(review.id)).not.toBe(null), WAIT);
    await unwatch();
    await vi.waitFor(() => expect(rowFor(review.id)).toBe(null), WAIT);

    // While the unwatch is in flight: a list from before it, still watched.
    await land([review, other]);
    await vi.waitFor(() => expect(titleOf(other.id)).toBe("#8 Marker"), WAIT);
    expect(rowFor(review.id)).toBe(null);

    // Answered at 01:05; a list the sync layer asked for before that lands.
    await vi.waitFor(() => expect(answer).toBeTypeOf("function"), WAIT);
    answer({ issue: { ...review, watched: false, updated_at: "2026-09-24T01:05:00Z" } });
    await land([{ ...review, updated_at: "2026-09-24T01:04:00Z" }, { ...other, title: "Marker again" }]);
    await vi.waitFor(() => expect(titleOf(other.id)).toBe("#8 Marker again"), WAIT);
    expect(rowFor(review.id)).toBe(null);

    // Watched again after the unwatch, on the issue page: the row is back.
    await land([{ ...review, updated_at: "2026-09-24T01:10:00Z" }, other]);
    await vi.waitFor(() => expect(rowFor(review.id)).not.toBe(null), WAIT);
  });

  it("comes back if the bridge refuses the unwatch", async () => {
    let refuse;
    answers["issues.unwatch"] = () => new Promise((_, reject) => { refuse = reject; });
    await vi.waitFor(() => expect(rowFor(review.id)).not.toBe(null), WAIT);
    await unwatch();
    await vi.waitFor(() => expect(rowFor(review.id)).toBe(null), WAIT);
    await vi.waitFor(() => expect(refuse).toBeTypeOf("function"), WAIT);
    refuse(new Error("Build cannot stop watching that issue."));
    await vi.waitFor(() => expect(rowFor(review.id)).not.toBe(null), WAIT);
    const held = await modules.tracker.readIssuesRecord(DEVICE, PROJECT);
    expect(held.issues[0].watched).toBe(true);
  });
});
