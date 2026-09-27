/** @vitest-environment jsdom */
// "Done since you left", with nothing mocked between the bridge and the tab:
// the bridge's real greeting gates it, and real `tasks.list` answers and the
// real push item drive it through the cache.
//
// The Rust side (bridge app::tests::user_session) runs two clients. The user
// left eight hours before a task was finished; the laptop holds that. The
// user comes back on a phone, the laptop is pushed the new session, and later
// the user comments. This tab is the laptop, and its clock runs six hours
// ahead of the bridge's, which must change nothing.

import { execFile } from "node:child_process";
import { resolve } from "node:path";
import { promisify } from "node:util";
import { afterEach, beforeAll, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { bridgeCapabilities, dispatchChangeEvent, greetBridge, resetChangeEvents } = await import("../src/core/changeEvents.js");
const { mountTasksPane } = await import("../src/core/trackerTasksPane.js");
const { readUserSession, userSessionAddress, writeUserSession } = await import("../src/core/userSessionCache.js");
const { deleteCached } = await import("../src/core/localCache.js");
const { tasksAddress, tasksRecord, writeTasksRecord } = await import("../src/core/trackerCache.js");

const run = promisify(execFile);
const bridgeRoot = resolve(process.cwd(), "../bridge");
const HOUR_MS = 60 * 60 * 1000;

/** One marked line a Rust test prints, run on demand so plain
 *  `npx vitest run` needs no prepared file. */
async function printed(testName, envName, marker) {
  const { stdout } = await run("cargo", ["test", "--lib", testName, "--", "--nocapture"], {
    cwd: bridgeRoot,
    env: { ...process.env, [envName]: "1" },
    maxBuffer: 2 * 1024 * 1024,
    timeout: 600_000,
  });
  const line = stdout.split(/\r?\n/).find((candidate) => candidate.includes(`${marker}=`));
  expect(line, `Rust must print ${marker}`).toBeDefined();
  return JSON.parse(line.slice(line.indexOf(`${marker}=`) + marker.length + 1));
}

let greeting;
let answers;
let host;
let pane;

beforeAll(async () => {
  const hello = await printed("the_greeting_and_the_probe_report_the_api_version", "BUILD_PRINT_HELLO_CAPABILITIES", "BUILD_REAL_HELLO");
  greeting = hello.result;
  answers = await printed(
    "the_answers_the_dashboard_reads_around_an_absence",
    "BUILD_PRINT_DONE_SINCE_LEFT",
    "BUILD_DONE_SINCE_LEFT",
  );
}, 1_220_000);

/** What the bridge answers `tasks.list` with right now. */
let listed;

beforeEach(() => {
  resetChangeEvents();
  globalThis.indexedDB = new IDBFactory();
  document.body.innerHTML = '<div id="tasks"></div>';
  host = document.querySelector("#tasks");
  listed = answers.away;
  // This device's clock, six hours ahead of the bridge's.
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(answers.away.user_session.now_ms + 6 * HOUR_MS);
});

afterEach(() => {
  pane?.dispose();
  vi.useRealTimers();
  resetChangeEvents();
});

const doneTab = () => host.querySelector('[data-dashboard-tab="done"]');
const doneRows = () => [...host.querySelectorAll('[data-dashboard-section="done"] .task-dashboard-row')]
  .map((row) => row.dataset.task);
const doneTitles = () => [...host.querySelectorAll('[data-dashboard-section="done"] .task-dashboard-group-title')]
  .map((title) => title.textContent);

async function mountWith(hello) {
  const call = async (method) => {
    if (method === "session.hello") return hello;
    if (method === "tasks.list") return listed;
    if (method === "tasks.columns") return { columns: [] };
    return {};
  };
  await greetBridge(call, { deviceId: "dev-1", strict: true });
  const projectId = answers.away.project_id;
  pane = mountTasksPane(host, {
    projectId,
    projectName: "Build",
    deviceId: "dev-1",
    projectKey: `dev-1|${projectId}`,
    callRpc: call,
    catalog: () => ({ providers: [] }),
    refreshCatalog: async () => ({ providers: [] }),
    feed: () => ({ workspaces: [], items: [], projects: [] }),
    defaultView: "dashboard",
    navigate: () => {},
  });
  await vi.waitFor(() => expect(doneTab()).not.toBeNull());
  doneTab().click();
}

async function remount() {
  pane.dispose();
  host.innerHTML = "";
  await mountWith(greeting);
}

/** The push the bridge sent the laptop, delivered as it arrives off the wire. */
const pushed = () => dispatchChangeEvent({ type: "changes", items: [answers.pushed] }, "dev-1");
const heldStart = async () => (await readUserSession("dev-1"))?.session_started_ms;

it("shows work finished while the user was away, across their return on another client, until they leave again", async () => {
  const finished = answers.away.tasks[0];
  expect(finished.status).toBe("done");
  await mountWith(greeting);
  expect(bridgeCapabilities("dev-1").tasks.doneSinceLeft).toBe(true);
  expect(doneTab().querySelector("span").textContent).toBe("Done");
  await vi.waitFor(() => expect(doneRows()).toEqual([finished.id]));
  expect(doneTitles()).toEqual(["While you were away"]);

  // The user came back on the phone: the push brings the new session here.
  listed = answers.back;
  expect(pushed()).toBe(true);
  await vi.waitFor(async () => expect(await heldStart()).toBe(answers.back.user_session.session_started_ms));
  expect(doneRows()).toEqual([finished.id]);
  // Finished before the session the phone started, however the clocks read.
  expect(doneTitles()).toEqual(["While you were away"]);

  // The user works on the phone for the next six hours, which moves nothing
  // the laptop holds, and the laptop repaints from its cache before the bridge
  // answers anything: six hours since its read is not six hours of silence.
  vi.setSystemTime(Date.now() + 6 * HOUR_MS);
  let answerLate;
  listed = new Promise((answer) => { answerLate = answer; });
  await remount();
  await vi.waitFor(() => expect(doneRows()).toEqual([finished.id]));
  answerLate(answers.back);

  // They comment on it, then leave for seven hours; the laptop reloads.
  listed = answers.commented;
  pushed();
  await vi.waitFor(async () => expect((await readUserSession("dev-1"))?.last_activity_ms)
    .toBe(answers.commented.user_session.last_activity_ms));
  // Reloaded at once: this device's clock says six hours have passed since
  // the comment, the bridge's says none. Still the same session.
  await remount();
  await vi.waitFor(() => expect(doneRows()).toEqual([finished.id]));

  // Seven hours on, the bridge answers with its own clock seven hours on.
  vi.setSystemTime(Date.now() + 7 * HOUR_MS);
  const later = answers.commented.user_session.now_ms + 7 * HOUR_MS;
  listed = { ...answers.commented, user_session: { ...answers.commented.user_session, now_ms: later } };
  await remount();
  await vi.waitFor(() => expect(host.querySelector(".task-dashboard-empty")?.textContent)
    .toBe("Nothing has moved to Done since you left."));
  expect(doneRows()).toEqual([]);
});

// A bridge that does not announce it answers with no session, and this device
// holds none: the Done it paints is the 24-hour one.
it("keeps the 24-hour Done when the bridge does not announce it", async () => {
  const hello = { ...greeting, capabilities: greeting.capabilities.filter((name) => name !== "tasks.doneSinceLeft") };
  const { user_session: _session, ...unsessioned } = answers.away;
  listed = unsessioned;
  // The cache outlives each case's fresh factory: drop what the last one held.
  await deleteCached([userSessionAddress("dev-1")]);
  await mountWith(hello);
  expect(bridgeCapabilities("dev-1").tasks.doneSinceLeft).toBe(false);
  await vi.waitFor(() => expect(host.querySelector('[data-dashboard-section="done"] .task-dashboard-row, .task-dashboard-empty'))
    .not.toBeNull());
  expect(await readUserSession("dev-1")).toBe(null);
  expect(doneTitles()).not.toContain("While you were away");
  expect(host.querySelector(".task-dashboard-empty")?.textContent).not.toBe("Nothing has moved to Done since you left.");
});

// A rollback (#104 review): a newer bridge left its session here, then an older
// one answers the list with no session and no `done_at`. The held session must
// not stand in for a bridge that no longer carries it — Done falls back to the
// 24-hour timeline rather than cutting every row off.
it("falls back to the 24-hour Done once an older bridge answers the list", async () => {
  await mountWith(greeting);
  await vi.waitFor(() => expect(doneTitles()).toEqual(["While you were away"]));
  expect(await readUserSession("dev-1")).not.toBe(null);

  const hello = { ...greeting, capabilities: greeting.capabilities.filter((name) => name !== "tasks.doneSinceLeft") };
  const { user_session: _session, ...older } = answers.away;
  listed = { ...older, tasks: older.tasks.map(({ done_at: _doneAt, ...task }) => task) };
  pane.dispose();
  host.innerHTML = "";
  await mountWith(hello);

  // The 24-hour Done reads the cached timelines, and this bridge was asked for
  // none: its empty line says which Done is painted.
  await vi.waitFor(() => expect(host.querySelector(".task-dashboard-empty")?.textContent)
    .toBe("Nothing moved to Done in the last 24 hours."));
  expect(await readUserSession("dev-1")).toBe(null);
  expect(doneRows()).toEqual([]);
});

// Paint from cache (#104 review): the session this device holds says the
// bridge carries it, so a cold start paints the Done it will keep, with no
// greeting and nothing answered.
it("paints Done since you left off the cache before any bridge answers", async () => {
  const projectId = answers.away.project_id;
  const finished = answers.away.tasks[0];
  // The cache outlives each case's fresh factory: hold only what this reload has.
  await deleteCached([userSessionAddress("dev-1"), tasksAddress("dev-1", projectId)]);
  await writeUserSession("dev-1", answers.away);
  await writeTasksRecord("dev-1", projectId, tasksRecord(answers.away.tasks, []));
  pane = mountTasksPane(host, {
    projectId,
    projectName: "Build",
    deviceId: "dev-1",
    projectKey: `dev-1|${projectId}`,
    callRpc: () => new Promise(() => {}),
    catalog: () => ({ providers: [] }),
    refreshCatalog: async () => ({ providers: [] }),
    feed: () => ({ workspaces: [], items: [], projects: [] }),
    defaultView: "dashboard",
    navigate: () => {},
  });
  expect(bridgeCapabilities("dev-1").tasks.doneSinceLeft).toBe(false);
  await vi.waitFor(() => expect(doneTab()).not.toBeNull());
  doneTab().click();
  await vi.waitFor(() => expect(doneRows()).toEqual([finished.id]));
  await vi.waitFor(() => expect(doneTitles()).toEqual(["While you were away"]));
});
