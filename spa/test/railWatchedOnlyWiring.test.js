/** @vitest-environment jsdom */
// #105, with nothing mocked between the greeting and the strip: the bridge's
// greeting (fixtures/api/v1/session.hello.json, which bridge/tests pins to the
// real reply) goes down the real greeting path, the real shell stands the rail
// up from a route whose link names an agent the reader does not watch, and the
// strip, the overview and the watch switch all read the real cache.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const greeting = JSON.parse(readFileSync(resolve(process.cwd(), "../fixtures/api/v1/session.hello.json"), "utf8")).result;
const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const agent = (id, ordinal, watched, name) => ({ id, ordinal, name, topic: name, provider: "claude_adk",
  state: "live", working: false, unread_count: 0, watched });
const branchRow = (quietWatched = false) => ({
  kind: "branch",
  project_id: "p1",
  branch: "build/login",
  run_id: "run-1",
  worktree_id: "wt-1",
  agents: [agent("ag-watched", 1, true, "Watched agent"), agent("ag-quiet", 2, quietWatched, "Quiet agent")],
  run: { run_id: "run-1", thread: { items: [], sessions: [] } },
});

let App;
let call;
let context;
let stopShell;
let resetDeviceContexts;
let resetChangeEvents;
let writeCached;

const sessionOnDevice = () => ({
  deviceId: "dev-1",
  call: (...args) => call(...args),
  close: () => {},
  peer: () => {},
  onCarrier: () => {},
});

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  document.body.innerHTML = bodyHtml;
  ({ App } = await import("../src/app.js"));
  ({ resetChangeEvents } = await import("../src/core/changeEvents.js"));
  resetChangeEvents();
  const { adoptDeviceSession, ...contexts } = await import("../src/core/deviceContexts.js");
  resetDeviceContexts = contexts.resetDeviceContexts;
  App.devices = [{ id: "dev-1", name: "studio", status: "online" }];
  let DEVICES_ADDRESS;
  ({ DEVICES_ADDRESS, writeCached } = await import("../src/core/localCache.js"));
  await writeCached(DEVICES_ADDRESS, App.devices);
  await (await import("../src/devices.js")).readCachedDevices();
  App.selectedDeviceId = "dev-1";
  call = vi.fn(async (method, params) => (method === "conversation.watch"
    ? { agent_id: params.agent_id, watched: true } : {}));
  context = adoptDeviceSession(sessionOnDevice());
  ({ stopShell } = await import("../src/core/shell.js"));
});

afterEach(async () => {
  stopShell();
  (await import("../src/core/taskFeed.js")).stopFeed();
  resetDeviceContexts();
  resetChangeEvents();
});

/** Greet the device's session the way connection.js does. */
async function greetSession() {
  const { adoptBridgeSelection, greetingInFlight } = await import("../src/core/deviceContexts.js");
  const { greetBridge } = await import("../src/core/changeEvents.js");
  const authority = greetingInFlight(context);
  const rawCall = async (method) => (method === "session.hello" ? greeting : {});
  return greetBridge(rawCall, {
    deviceId: "dev-1",
    strict: true,
    isCurrent: () => authority.current(),
    install: (selection) => {
      const adapter = selection.unsupported ? null : selection.create(rawCall);
      adoptBridgeSelection(context, selection, adapter, authority);
      return adapter;
    },
  });
}

/** What the sync layer writes for this branch: its board and its row. */
async function cacheBranch(row) {
  const { liveFeedSnapshot } = await import("../src/core/feedMerge.js");
  const projects = [{ project_id: "p1", name: "notes", is_git: true }];
  const view = liveFeedSnapshot({ items: [row], runs: [] }, { projects }, { workspaces: [] }, "dev-1");
  await writeCached({ deviceId: "dev-1", entityId: "", kind: "feed" }, view);
  await writeCached({ deviceId: "dev-1", entityId: "", kind: "projects" }, view.projects);
  await (await import("./railCacheFixture.js")).writeRailRow(row);
}

const rail = () => document.getElementById("agent-rail");
const stripIds = () => [...rail().querySelectorAll('.rail-bubble[data-bubble="agent"]')].map((node) => node.dataset.agent);
const bubbleOf = (agentId) => rail().querySelector(`.rail-bubble[data-agent="${agentId}"]`);

it("opens a linked unwatched agent on a temporary bubble, finds it under Not watching, and keeps it once watched", async () => {
  await greetSession();
  await cacheBranch(branchRow());
  await (await import("../src/core/taskFeed.js")).startFeed();
  App.route = { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login", tab: "changes", agent: "ag-quiet" };
  const { standShell } = await import("../src/core/shell.js");
  standShell(App.route);

  await vi.waitFor(() => {
    expect(stripIds()).toEqual(["ag-watched", "ag-quiet"]);
    expect(bubbleOf("ag-quiet").classList.contains("active")).toBe(true);
    expect(bubbleOf("ag-quiet").classList.contains("rail-bubble-unwatched")).toBe(true);
    expect(rail().querySelector(".rail-watch")?.getAttribute("aria-pressed")).toBe("false");
  });

  bubbleOf("ag-watched").click();
  await vi.waitFor(() => expect(stripIds()).toEqual(["ag-watched"]));

  rail().querySelector(".rail-overview-toggle").click();
  const quietRow = () => rail().querySelector('.rail-overview-group[aria-label="Not watching"] [data-overview-agent="ag-quiet"]');
  await vi.waitFor(() => expect(quietRow()).toBeTruthy());
  expect(rail().querySelector('.rail-overview-section [data-overview-agent="ag-watched"]').closest(".rail-overview-group")).toBeNull();
  quietRow().click();
  await vi.waitFor(() => expect(bubbleOf("ag-quiet")?.classList.contains("active")).toBe(true));

  // The bridge's answer is written into the cached row, so leaving before the
  // push that follows it lands keeps the bubble all the same.
  const { readCached } = await import("../src/core/localCache.js");
  const cachedWatch = async () => (await readCached({ deviceId: "dev-1", entityId: "run-1", kind: "row" }))
    ?.value?.agents?.find((one) => one.id === "ag-quiet")?.watched;
  rail().querySelector(".rail-watch").click();
  await vi.waitFor(() => expect(call).toHaveBeenCalledWith("conversation.watch", { entity_id: "run-1", agent_id: "ag-quiet" }));
  await vi.waitFor(async () => expect(await cachedWatch()).toBe(true));
  await vi.waitFor(() => expect(bubbleOf("ag-quiet").classList.contains("rail-bubble-unwatched")).toBe(false));

  bubbleOf("ag-watched").click();
  await vi.waitFor(() => expect(bubbleOf("ag-watched").classList.contains("active")).toBe(true));
  expect(stripIds()).toEqual(["ag-watched", "ag-quiet"]);
});
