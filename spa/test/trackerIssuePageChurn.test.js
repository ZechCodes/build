/** @vitest-environment jsdom */
// #126, #119's cause on one issue's page: pushes arriving faster than an
// `issues.get` round trip. Each push used to start a fresh read and drop every
// answer a newer read had overtaken, so while the agents kept commenting no
// answer ever landed and the page stayed stale. Real page and cache; the push
// is the page's own registration, fired the way core/changeEvents.js fires it.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, issue } from "./trackerWireFixture.js";

let watchers = [];
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["issues"] }, issues: {} }),
  watchChanges: (registration) => {
    watchers.push(registration);
    return { dispose: () => {} };
  },
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));

const DEVICE = "dev-126";
const PROJECT = "proj-1";
const ISSUE = "issue-126";
const before = issue({ id: ISSUE, number: 126, title: "Before the move", status: "in_review" });
const after = { ...before, title: "After the move", status: "done" };

let host, page, answers, reads;
const answer = (at, which) => answers[at]({ issue: which, timeline: [] });
const push = () => watchers
  .filter((one) => one.entity === PROJECT)
  .forEach((one) => one.onChanges([{ issues: { issue_ids: [ISSUE] } }]));
const title = () => host.querySelector(".issue-page-title")?.textContent;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  watchers = [];
  reads = 0;
  // Each read waits until the case answers it, as a slow wire does.
  answers = [];
  document.body.innerHTML = '<div id="pane"></div>';
  host = document.querySelector("#pane");
  const trackerCache = await import("../src/core/trackerCache.js");
  await trackerCache.writeIssuesRecord(DEVICE, PROJECT, trackerCache.issuesRecord([before], columns()));
  const { mountIssuePage } = await import("../src/core/trackerIssuePage.js");
  page = mountIssuePage(host, {
    deviceId: DEVICE,
    projectId: PROJECT,
    projectKey: `${DEVICE}|${PROJECT}`,
    issueId: ISSUE,
    callRpc: (method) => {
      if (method !== "issues.get") return new Promise(() => {});
      reads += 1;
      return new Promise((resolve) => answers.push(resolve));
    },
    catalog: () => ({ providers: [] }),
    refreshCatalog: async () => ({ providers: [] }),
    feed: () => ({ items: [], workspaces: [] }),
    navigate: () => {},
  });
});

afterEach(() => page?.dispose());

it("lands the read that is out while pushes keep coming, and reads once more after it", async () => {
  await vi.waitFor(() => expect(answers).toHaveLength(1)); // the mount's own read
  answer(0, before);
  await vi.waitFor(() => expect(title()).toBe("Before the move"));

  push(); // the move
  await vi.waitFor(() => expect(answers).toHaveLength(2));
  push(); // and the comments after it, each before the read has come back
  push();
  push();
  expect(reads).toBe(2);

  answer(1, after);
  await vi.waitFor(() => expect(title()).toBe("After the move"));

  // One read after the burst, not one per push.
  await vi.waitFor(() => expect(answers).toHaveLength(3));
  expect(reads).toBe(3);
  answer(2, after);
});
