/** @vitest-environment jsdom */
// Keep the bridge's real session.hello and the SPA's greeting, adapter, gate,
// and issue page on one path. A names-only greeting must reach the watch
// control without a minor-version guess making up a missing capability.

import { execFile } from "node:child_process";
import { resolve } from "node:path";
import { promisify } from "node:util";
import { afterEach, beforeAll, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, comment, issue } from "./trackerWireFixture.js";
import versions from "../../fixtures/api/versions.json";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { bridgeCapabilities, greetBridge, resetChangeEvents } = await import("../src/core/changeEvents.js");
const { carriesWatching } = await import("../src/core/trackerWatch.js");
const { mountIssuePage } = await import("../src/core/trackerIssuePage.js");
const { writeIssuesRecord } = await import("../src/core/trackerCache.js");

const run = promisify(execFile);
const bridgeRoot = resolve(process.cwd(), "../bridge");
const issueAnswer = {
  issue: issue({ id: "issue-1", number: 122, watched: false }),
  timeline: [comment({ id: "ic-1", issue_id: "issue-1" })],
};
let realGreeting;
let host;
let page;
let calls;

beforeAll(async () => {
  // The Rust test calls the real FrameHandler and prints the wire envelope.
  // Generate it on demand so plain `npx vitest run` needs no prepared file.
  const { stdout } = await run("cargo", [
    "test", "--lib", "the_greeting_and_the_probe_report_the_api_version", "--", "--nocapture",
  ], {
    cwd: bridgeRoot,
    env: { ...process.env, BUILD_PRINT_HELLO_CAPABILITIES: "1" },
    maxBuffer: 2 * 1024 * 1024,
    timeout: 600_000,
  });
  const marked = stdout.split(/\r?\n/).find((line) => line.includes("BUILD_REAL_HELLO="));
  expect(marked, "Rust must emit the actual session.hello reply").toBeDefined();
  const envelope = JSON.parse(marked.slice(marked.indexOf("BUILD_REAL_HELLO=") + "BUILD_REAL_HELLO=".length));
  expect(envelope.ok).toBe(true);
  realGreeting = envelope.result;
  expect(realGreeting.api_version).toBe(versions.current);
  expect(realGreeting.capabilities).toContain("issues.watching");
}, 610_000);

beforeEach(async () => {
  resetChangeEvents();
  globalThis.indexedDB = new IDBFactory();
  document.body.innerHTML = '<div id="issue"></div>';
  host = document.querySelector("#issue");
  await writeIssuesRecord("dev-1", "proj-1", { issues: [], columns: columns() });
  calls = [];
  page = null;
});

afterEach(() => {
  page?.dispose();
  resetChangeEvents();
});

async function mountWith(greeting) {
  const call = async (method, params) => {
    calls.push([method, params]);
    if (method === "session.hello") return greeting;
    if (method === "issues.get") return issueAnswer;
    return {};
  };
  await greetBridge(call, { deviceId: "dev-1", strict: true });
  page = mountIssuePage(host, {
    projectId: "proj-1", deviceId: "dev-1", projectKey: "dev-1|proj-1", issueId: "issue-1",
    callRpc: call,
    catalog: () => ({ providers: [] }),
    refreshCatalog: async () => ({ providers: [] }),
    feed: () => ({ workspaces: [], items: [], projects: [] }),
    navigate: () => {},
  });
  await vi.waitFor(() => expect(host.querySelector(".issue-page-title")).not.toBeNull());
}

it("renders and wires watching from the real names-only bridge greeting", async () => {
  await mountWith(realGreeting);
  expect(bridgeCapabilities("dev-1").issues.watching).toBe(true);
  expect(carriesWatching("dev-1")).toBe(true);
  const button = host.querySelector(".rail-watch");
  expect(button).not.toBeNull();
  button.click();
  await vi.waitFor(() => expect(calls).toContainEqual(["issues.watch", { issue_id: "issue-1" }]));
});

it("hides watching when the same bridge version omits its name", async () => {
  const greeting = {
    ...realGreeting,
    capabilities: realGreeting.capabilities.filter((name) => name !== "issues.watching"),
  };
  await mountWith(greeting);
  expect(bridgeCapabilities("dev-1").issues.watching).toBe(false);
  expect(carriesWatching("dev-1")).toBe(false);
  expect(host.querySelector(".rail-watch")).toBeNull();
  expect(calls.some(([method]) => method === "issues.read_through")).toBe(false);
});
