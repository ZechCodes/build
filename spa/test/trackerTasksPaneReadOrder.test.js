/** @vitest-environment jsdom */
// #129: the Tasks tab picks between the whole list and its own filtered
// answer by when the bridge was ASKED for each, not by when each reached the
// browser. The background pass's whole-list read is large, so one asked
// before the tab's small filtered read can land after it — and it is still
// the older news. Real pane and cache; the wire is the only stand-in.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, task } from "./trackerWireFixture.js";

vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["tasks"] }, tasks: {} }),
  watchChanges: () => ({ dispose: () => {} }),
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));

const DEVICE = "dev-129";
const PROJECT = "proj-1";
const reviewing = task({ id: "task-101", number: 101, title: "Wire 1.14.0", status: "in_review" });
const moved = { ...reviewing, status: "done" };
// Only the whole list names this one, so its label in the menus says the tab
// has taken that list in.
const onlyInTheWholeList = task({ id: "task-102", number: 102, title: "Menus", labels: ["whole-list"] });

let host, pane, panes, answers, trackerCache, nextTaskRead;

const needsYouTitles = () => [...host.querySelectorAll('[data-task-group="needsYou"] .task-title')]
  .map((one) => one.textContent);
const labelsOffered = () => {
  const menu = host.querySelector('[data-filter-menu="label"]');
  const press = menu.querySelector(".fmenu-press");
  if (press.getAttribute("aria-expanded") !== "true") press.click();
  return [...menu.querySelectorAll(".fmenu-row")].map((one) => one.dataset.value);
};
/** The background pass's whole list, asked as read `asked`, landing now. */
const wholeListLands = (tasks, asked) =>
  trackerCache.writeTasksRecord(DEVICE, PROJECT, trackerCache.tasksRecord(tasks, columns(), asked));

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  answers = [];
  panes = [];
  document.body.innerHTML = '<div id="tasks"></div>';
  host = document.querySelector("#tasks");
  trackerCache = await import("../src/core/trackerCache.js");
  ({ nextTaskRead } = await import("../src/core/taskReadOrder.js"));
  await trackerCache.writeTasksRecord(DEVICE, PROJECT, trackerCache.tasksRecord([reviewing], columns()));
});

afterEach(() => {
  vi.restoreAllMocks();
  panes.forEach((one) => one.dispose());
});

/** A pane on `target`, whose `tasks.list` reads wait in `asked` until the
 *  case answers them, and whose every other call never answers. */
async function mountPane(target, view, asked) {
  const { mountTasksPane } = await import("../src/core/trackerTasksPane.js");
  const mounted = mountTasksPane(target, {
    deviceId: DEVICE,
    projectId: PROJECT,
    projectName: "Build",
    projectKey: `${DEVICE}|${PROJECT}`,
    defaultView: view,
    callRpc: (method) => (method === "tasks.list" ? new Promise((resolve) => asked.push(resolve)) : new Promise(() => {})),
    catalog: () => ({ providers: [] }),
    feed: () => ({ items: [], workspaces: [] }),
  });
  panes.push(mounted);
  await vi.waitFor(() => expect(asked).toHaveLength(1));
  return mounted;
}

// The list view's default filter (open tasks) narrows the read, so the
// tab's answer has a record of its own beside the whole list.
async function mount() {
  pane = await mountPane(host, "list", answers);
}

it("keeps its filtered answer over a whole list asked before it that lands after it", async () => {
  const wholeAsked = await nextTaskRead(); // the background pass asks first
  await mount(); // then the tab asks
  answers[0]({ tasks: [moved] });
  await vi.waitFor(() => expect(needsYouTitles()).toEqual([]));

  await wholeListLands([reviewing, onlyInTheWholeList], wholeAsked);

  await vi.waitFor(() => expect(labelsOffered()).toContain("whole-list"));
  expect(needsYouTitles()).toEqual([]);
});

it("takes a whole list asked after its filtered answer", async () => {
  await mount();
  answers[0]({ tasks: [reviewing] });
  await vi.waitFor(() => expect(needsYouTitles()).toEqual(["Wire 1.14.0"]));

  await wholeListLands([moved, onlyInTheWholeList], await nextTaskRead());

  await vi.waitFor(() => expect(needsYouTitles()).toEqual([]));
});

// A card moved, or a task filed, is a list written here rather than read,
// and it is newer than every read asked before it — even on a clock too coarse
// to tell them apart (#129 review). Two panes on one project: the Dashboard's
// unnarrowed answer, and a board whose own narrowed read is a different record.
it("takes a card another pane moved over its own older answer, on a clock that does not move", async () => {
  vi.spyOn(Date, "now").mockReturnValue(Date.parse("2026-09-26T04:00:00Z"));
  document.body.innerHTML = '<div id="dashboard"></div><div id="board"></div>';
  const dashboard = document.querySelector("#dashboard");
  const board = document.querySelector("#board");
  const dashboardTitles = () => [...dashboard.querySelectorAll(".task-dashboard-title")].map((one) => one.textContent);

  await nextTaskRead(); // some read earlier in the same millisecond
  const dashboardReads = [];
  await mountPane(dashboard, "dashboard", dashboardReads);
  dashboardReads[0]({ tasks: [reviewing] });
  await vi.waitFor(() => expect(dashboardTitles()).toEqual(["Wire 1.14.0"]));

  const boardReads = [];
  await mountPane(board, "board", boardReads);
  boardReads[0]({ tasks: [reviewing] });
  const card = () => board.querySelector('.task-card[data-task="task-101"]');
  await vi.waitFor(() => expect(card()?.dataset.status).toBe("in_review"));

  card().dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true })); // to Done

  await vi.waitFor(() => expect(dashboardTitles()).toEqual([]));
});
