/** @vitest-environment jsdom */
// "Done since you left", with nothing mocked between the bridge and the tab:
// the bridge's real greeting gates it, and two real `issues.list` answers,
// one before and one after a user action, drive it through the cache.
//
// The Rust side (bridge app::tests::user_session) has an agent finish an issue
// and then the user comment on the bridge. Seven hours later the finish was
// news before that comment and is not after it: the user was here since.

import { execFile } from "node:child_process";
import { resolve } from "node:path";
import { promisify } from "node:util";
import { afterEach, beforeAll, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { bridgeCapabilities, greetBridge, resetChangeEvents } = await import("../src/core/changeEvents.js");
const { mountIssuesPane } = await import("../src/core/trackerIssuesPane.js");
const { writeUserSession } = await import("../src/core/userSessionCache.js");

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
    "a_user_action_after_work_finished_moves_the_answer_the_dashboard_reads",
    "BUILD_PRINT_DONE_SINCE_LEFT",
    "BUILD_DONE_SINCE_LEFT",
  );
}, 1_220_000);

beforeEach(() => {
  resetChangeEvents();
  globalThis.indexedDB = new IDBFactory();
  document.body.innerHTML = '<div id="issues"></div>';
  host = document.querySelector("#issues");
  // Seven hours after the user's comment: they have left and come back.
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(answers.after.user_session.last_activity_ms + 7 * HOUR_MS);
});

afterEach(() => {
  pane?.dispose();
  vi.useRealTimers();
  resetChangeEvents();
});

const doneTab = () => host.querySelector('[data-dashboard-tab="done"]');
const doneRows = () => [...host.querySelectorAll('[data-dashboard-section="done"] .issue-dashboard-row')]
  .map((row) => row.dataset.issue);

async function mountWith(hello, listAnswer) {
  const call = async (method) => {
    if (method === "session.hello") return hello;
    if (method === "issues.list") return listAnswer;
    if (method === "issues.columns") return { columns: [] };
    return {};
  };
  await greetBridge(call, { deviceId: "dev-1", strict: true });
  pane = mountIssuesPane(host, {
    projectId: listAnswer.project_id,
    projectName: "Build",
    deviceId: "dev-1",
    projectKey: `dev-1|${listAnswer.project_id}`,
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

it("shows work finished after the user left, and drops it once the bridge saw them after it", async () => {
  const finished = answers.before.issues[0];
  expect(finished.status).toBe("done");
  await mountWith(greeting, answers.before);
  expect(bridgeCapabilities("dev-1").issues.doneSinceLeft).toBe(true);
  expect(doneTab().textContent).toContain("Done since you left");
  await vi.waitFor(() => expect(doneRows()).toEqual([finished.id]));

  // What a later list read writes: the same session writer the pane and the
  // sync pass use, and the cache announcement is the repaint.
  await writeUserSession("dev-1", answers.after);
  await vi.waitFor(() => expect(doneRows()).toEqual([]));
  expect(host.querySelector(".issue-dashboard-empty").textContent).toBe("Nothing has moved to Done since you left.");
});

it("keeps the 24-hour Done when the bridge does not announce it", async () => {
  const hello = { ...greeting, capabilities: greeting.capabilities.filter((name) => name !== "issues.doneSinceLeft") };
  await mountWith(hello, answers.before);
  expect(bridgeCapabilities("dev-1").issues.doneSinceLeft).toBe(false);
  expect(doneTab().textContent.trim()).toMatch(/^Done\d*$/);
});
