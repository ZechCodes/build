/** @vitest-environment jsdom */
// #129: the Issues tab picks between the whole list and its own filtered
// answer by when the bridge was ASKED for each, not by when each reached the
// browser. The background pass's whole-list read is large, so one asked
// before the tab's small filtered read can land after it — and it is still
// the older news. Real pane and cache; the wire is the only stand-in.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, issue } from "./trackerWireFixture.js";

vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["issues"] }, issues: {} }),
  watchChanges: () => ({ dispose: () => {} }),
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));

const DEVICE = "dev-129";
const PROJECT = "proj-1";
const reviewing = issue({ id: "issue-101", number: 101, title: "Wire 1.14.0", status: "in_review" });
const moved = { ...reviewing, status: "done" };
// Only the whole list names this one, so its label in the menus says the tab
// has taken that list in.
const onlyInTheWholeList = issue({ id: "issue-102", number: 102, title: "Menus", labels: ["whole-list"] });

let host, pane, answers, trackerCache, nextIssueRead;

const needsYouTitles = () => [...host.querySelectorAll('[data-issue-group="needsYou"] .issue-title')]
  .map((one) => one.textContent);
const labelsOffered = () => {
  const menu = host.querySelector('[data-filter-menu="label"]');
  const press = menu.querySelector(".fmenu-press");
  if (press.getAttribute("aria-expanded") !== "true") press.click();
  return [...menu.querySelectorAll(".fmenu-row")].map((one) => one.dataset.value);
};
/** The background pass's whole list, asked as read `asked`, landing now. */
const wholeListLands = (issues, asked) =>
  trackerCache.writeIssuesRecord(DEVICE, PROJECT, trackerCache.issuesRecord(issues, columns(), asked));

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  answers = [];
  document.body.innerHTML = '<div id="issues"></div>';
  host = document.querySelector("#issues");
  trackerCache = await import("../src/core/trackerCache.js");
  ({ nextIssueRead } = await import("../src/core/issueReadOrder.js"));
  await trackerCache.writeIssuesRecord(DEVICE, PROJECT, trackerCache.issuesRecord([reviewing], columns()));
});

afterEach(() => pane?.dispose());

async function mount() {
  const { mountIssuesPane } = await import("../src/core/trackerIssuesPane.js");
  // The list view's default filter (open issues) narrows the read, so the
  // tab's answer has a record of its own beside the whole list.
  pane = mountIssuesPane(host, {
    deviceId: DEVICE,
    projectId: PROJECT,
    projectName: "Build",
    projectKey: `${DEVICE}|${PROJECT}`,
    defaultView: "list",
    callRpc: (method) => (method === "issues.list" ? new Promise((resolve) => answers.push(resolve)) : new Promise(() => {})),
    catalog: () => ({ providers: [] }),
    feed: () => ({ items: [], workspaces: [] }),
  });
  await vi.waitFor(() => expect(answers).toHaveLength(1));
}

it("keeps its filtered answer over a whole list asked before it that lands after it", async () => {
  const wholeAsked = await nextIssueRead(); // the background pass asks first
  await mount(); // then the tab asks
  answers[0]({ issues: [moved] });
  await vi.waitFor(() => expect(needsYouTitles()).toEqual([]));

  await wholeListLands([reviewing, onlyInTheWholeList], wholeAsked);

  await vi.waitFor(() => expect(labelsOffered()).toContain("whole-list"));
  expect(needsYouTitles()).toEqual([]);
});

it("takes a whole list asked after its filtered answer", async () => {
  await mount();
  answers[0]({ issues: [reviewing] });
  await vi.waitFor(() => expect(needsYouTitles()).toEqual(["Wire 1.14.0"]));

  await wholeListLands([moved, onlyInTheWholeList], await nextIssueRead());

  await vi.waitFor(() => expect(needsYouTitles()).toEqual([]));
});
