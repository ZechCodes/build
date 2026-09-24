/** @vitest-environment jsdom */
// "Done since you left", with nothing mocked between the bridge and the tab:
// the bridge's real greeting gates it, and real `issues.list` answers and the
// real push item drive it through the cache.
//
// The Rust side (bridge app::tests::user_session) runs two clients. The user
// left eight hours before an issue was finished; the laptop holds that. The
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
const { mountIssuesPane } = await import("../src/core/trackerIssuesPane.js");
const { readUserSession } = await import("../src/core/userSessionCache.js");

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

/** What the bridge answers `issues.list` with right now. */
let listed;

beforeEach(() => {
  resetChangeEvents();
  globalThis.indexedDB = new IDBFactory();
  document.body.innerHTML = '<div id="issues"></div>';
  host = document.querySelector("#issues");
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
const doneRows = () => [...host.querySelectorAll('[data-dashboard-section="done"] .issue-dashboard-row')]
  .map((row) => row.dataset.issue);

async function mountWith(hello) {
  const call = async (method) => {
    if (method === "session.hello") return hello;
    if (method === "issues.list") return listed;
    if (method === "issues.columns") return { columns: [] };
    return {};
  };
  await greetBridge(call, { deviceId: "dev-1", strict: true });
  const projectId = answers.away.project_id;
  pane = mountIssuesPane(host, {
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
  const finished = answers.away.issues[0];
  expect(finished.status).toBe("done");
  await mountWith(greeting);
  expect(bridgeCapabilities("dev-1").issues.doneSinceLeft).toBe(true);
  expect(doneTab().textContent).toContain("Done since you left");
  await vi.waitFor(() => expect(doneRows()).toEqual([finished.id]));

  // The user came back on the phone: the push brings the new session here.
  listed = answers.back;
  expect(pushed()).toBe(true);
  await vi.waitFor(async () => expect(await heldStart()).toBe(answers.back.user_session.session_started_ms));
  expect(doneRows()).toEqual([finished.id]);

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
  await vi.waitFor(() => expect(host.querySelector(".issue-dashboard-empty")?.textContent)
    .toBe("Nothing has moved to Done since you left."));
  expect(doneRows()).toEqual([]);
});

it("keeps the 24-hour Done when the bridge does not announce it", async () => {
  const hello = { ...greeting, capabilities: greeting.capabilities.filter((name) => name !== "issues.doneSinceLeft") };
  await mountWith(hello);
  expect(bridgeCapabilities("dev-1").issues.doneSinceLeft).toBe(false);
  expect(doneTab().textContent.trim()).toMatch(/^Done\d*$/);
});
