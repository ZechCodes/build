// @vitest-environment jsdom
// The rail's projects face, wired: the switch between the two faces, the
// blocks painted from the feed with their rows beneath them, the fold, the
// head that opens the project's checkout, the + on each block that opens the
// create surface, the new-project control at the top, the active block, and a
// Recent fold per block.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

let feedItems = [];
const feedProjects = [
  { id: "p1", name: "relaydb" },
  { id: "p2", name: "dotfiles" },
  { id: "p3", name: "mascot" },
];
let subscriber = null;
const refreshFeed = vi.fn(async () => subscriber && subscriber({ items: feedItems, projects: feedProjects }));
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => {
    subscriber = fn;
    fn({ items: feedItems, projects: feedProjects });
    return () => {};
  },
  startFeed: () => {},
  stopFeed: () => {},
  refreshFeed: (...args) => refreshFeed(...args),
  primaryRunIdFor: () => null,
}));
const openCreateWork = vi.fn();
vi.mock("../src/core/createWork.js", () => ({ openCreateWork: (...args) => openCreateWork(...args) }));
const openNewRepo = vi.fn();
vi.mock("../src/sheets/newRepo.js", () => ({ openNewRepo: (...args) => openNewRepo(...args) }));

let App;
let initInboxRail;
let setInboxView;

const flush = () => new Promise((done) => setTimeout(done, 0));
const list = () => document.getElementById("inbox-list");
const blocks = () => [...document.querySelectorAll("#inbox-list .inbox-project")];
const blockFor = (projectId) => document.querySelector(`.inbox-project[data-project="${projectId}"]`);
const rowsIn = (block) => [...block.querySelectorAll(".inbox-project-rows > .inbox-entry")].map((row) => row.dataset.key);
const rowFor = (entityId) => document.querySelector(`.inbox-entry[data-entity="${entityId}"]`);
const viewButton = (view) => document.querySelector(`[data-inbox-view="${view}"]`);

const now = () => new Date().toISOString();
const hoursAgo = (hours) => new Date(Date.now() - hours * 3600 * 1000).toISOString();

const branchRow = (over = {}) => ({
  kind: "branch",
  project_id: "p1",
  project: "relaydb",
  branch: "build/login",
  title: "Fix the login flow",
  state: "review",
  unread: true,
  unread_count: 1,
  unread_reason: "done",
  working: false,
  stat: null,
  anchor: hoursAgo(3),
  last_activity: now(),
  can_finish: true,
  finish: { warnings: [] },
  muted: false,
  run_id: "run-1",
  issue_id: null,
  primary: false,
  ...over,
});

const primaryRow = (over = {}) => ({
  kind: "branch",
  project_id: "p1",
  project: "relaydb",
  branch: "main",
  title: "relaydb",
  state: "idle",
  unread: false,
  unread_count: 0,
  working: false,
  stat: null,
  anchor: hoursAgo(50),
  last_activity: now(),
  can_finish: false,
  finish: { warnings: [] },
  muted: false,
  run_id: null,
  worktree_id: null,
  issue_id: null,
  primary: true,
  ...over,
});

const issueRow = (over = {}) => ({
  kind: "issue",
  project_id: "p2",
  project: "dotfiles",
  branch: null,
  title: "Rework the prompt cache",
  state: "plan_review",
  unread: false,
  unread_count: 0,
  working: true,
  stat: null,
  anchor: hoursAgo(1),
  last_activity: now(),
  can_finish: true,
  finish: { warnings: [] },
  muted: false,
  issue_id: "iss-1",
  implementation_active: false,
  primary: false,
  ...over,
});

const captureRow = (over = {}) => ({
  kind: "capture",
  capture_id: "cap-1",
  project_id: "",
  project: "",
  branch: null,
  issue_id: null,
  title: "fix the redirect",
  text: "fix the redirect",
  state: "routing",
  created_at: now(),
  anchor: hoursAgo(0.5),
  last_activity: now(),
  unread: false,
  unread_count: 0,
  routing: null,
  question: null,
  ...over,
});

const feed = (items) => {
  feedItems = items;
  subscriber({ items, projects: feedProjects });
};

/** Everything the DOM under `target` did while `act` ran. */
const churn = (target, act) => {
  const seen = [];
  const observer = new MutationObserver((records) => seen.push(...records));
  observer.observe(target, { childList: true, subtree: true, attributes: true, characterData: true });
  act();
  seen.push(...observer.takeRecords());
  observer.disconnect();
  return seen;
};

beforeEach(async () => {
  vi.resetModules();
  ({ App } = await import("../src/app.js"));
  ({ initInboxRail } = await import("../src/core/inboxShell.js"));
  ({ setInboxView } = await import("../src/core/inboxView.js"));
  document.body.innerHTML = bodyHtml;
  document.body.className = "";
  localStorage.clear();
  location.hash = "";
  window.innerWidth = 1200;
  App.route = { name: "inbox" };
  App.gated = false;
  App.call = vi.fn(async () => ({ ok: true }));
  refreshFeed.mockClear();
  openCreateWork.mockClear();
  openNewRepo.mockClear();
  feedItems = [branchRow(), primaryRow(), issueRow(), captureRow()];
  initInboxRail();
});

afterEach(() => {
  document.getElementById("confirm-scrim")?.remove();
});

describe("the switch between the two faces", () => {
  it("stands at the head's right edge, the inbox pressed to begin with", () => {
    const head = document.querySelector(".inbox-head");
    expect([...head.children].map((child) => child.id)).toEqual(["inbox-collapse", "", "inbox-views"]);
    expect(viewButton("inbox").getAttribute("aria-pressed")).toBe("true");
    expect(viewButton("projects").getAttribute("aria-pressed")).toBe("false");
    expect(list().dataset.view).toBe("inbox");
    expect(blocks()).toEqual([]);
  });

  it("switches to the projects face on a press, remembers it, and comes back up on it", async () => {
    viewButton("projects").click();
    expect(viewButton("projects").getAttribute("aria-pressed")).toBe("true");
    expect(viewButton("inbox").getAttribute("aria-pressed")).toBe("false");
    expect(list().dataset.view).toBe("projects");
    expect(localStorage.getItem("build.inbox.view")).toBe("projects");

    vi.resetModules();
    ({ initInboxRail } = await import("../src/core/inboxShell.js"));
    document.body.innerHTML = bodyHtml;
    initInboxRail();
    expect(viewButton("projects").getAttribute("aria-pressed")).toBe("true");
    expect(list().dataset.view).toBe("projects");
  });

  it("switching back paints the one list again, project tags and all", () => {
    setInboxView("projects");
    expect(rowFor("run-1").querySelector(".inbox-tag")).toBeNull();
    setInboxView("inbox");
    expect(blocks()).toEqual([]);
    expect(rowFor("run-1").querySelector(".inbox-tag").textContent).toBe("relaydb");
  });
});

describe("the projects face", () => {
  beforeEach(() => viewButton("projects").click());

  it("paints the unrouted captures first, then every project with its rows beneath it", () => {
    const loose = [...list().querySelectorAll(".inbox-unsorted > .inbox-entry")].map((row) => row.dataset.key);
    expect(loose).toEqual(["capture:cap-1"]);
    // relaydb's oldest row (the primary, 50h) beats dotfiles' (1h); mascot has nothing.
    expect(blocks().map((block) => block.dataset.project)).toEqual(["p1", "p2", "p3"]);
    expect(rowsIn(blockFor("p1"))).toEqual(["branch:p1:main", "run-1"]);
    expect(rowsIn(blockFor("p2"))).toEqual(["iss-1"]);
    expect(rowsIn(blockFor("p3"))).toEqual([]);
    expect(blockFor("p3").querySelector(".inbox-project-empty").textContent).toContain("Nothing here yet");
    expect(blockFor("p1").querySelector(".inbox-project-name").textContent).toBe("relaydb");
    expect(blockFor("p1").querySelector(".inbox-project-head .inbox-unread").textContent).toBe("1");
    // The block already says which project, so the row does not.
    expect(rowFor("run-1").querySelector(".inbox-tag")).toBeNull();
    // New project heads the list.
    expect(list().firstElementChild.matches("[data-new-project]")).toBe(true);
  });

  it("stands a project whose rows have all gone quiet after the ones with live work", () => {
    feed([
      branchRow({ anchor: hoursAgo(300), last_activity: hoursAgo(40) }),
      issueRow(),
      branchRow({ project_id: "p3", project: "mascot", branch: "build/model", run_id: "run-3", anchor: hoursAgo(2) }),
      captureRow(),
    ]);
    expect(blocks().map((block) => block.dataset.project)).toEqual(["p3", "p2", "p1"]);
    expect(rowsIn(blockFor("p1"))).toEqual([]);
    expect(blockFor("p1").querySelector("[data-recent-toggle]")).toBeTruthy();
  });

  it("highlights the block holding the branch or issue the route stands on", () => {
    expect(list().querySelector(".inbox-project.active")).toBeNull();
    App.route = { name: "branch", projectId: "p2", branch: "x", tab: "changes" };
    feed(feedItems);
    expect([...list().querySelectorAll(".inbox-project.active")].map((block) => block.dataset.project)).toEqual(["p2"]);
    App.route = { name: "capture", id: "cap-1" };
    feed(feedItems);
    expect(list().querySelector(".inbox-project.active")).toBeNull();
  });

  it("draws the folds and the Recent toggles as chevron icons", () => {
    expect(blockFor("p1").querySelector("[data-project-fold] svg")).toBeTruthy();
    feed([branchRow({ branch: "build/old", run_id: "run-old", anchor: hoursAgo(300), last_activity: hoursAgo(40) }), primaryRow()]);
    expect(blockFor("p1").querySelector("[data-recent-toggle] svg")).toBeTruthy();
  });

  it("opens a row like the inbox does", async () => {
    rowFor("run-1").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("entity.seen", { entity_id: "run-1" });
    expect(location.hash).toBe("#/project/p1/branch/build%2Flogin/changes");
  });

  it("opens the project's checkout from its name, and nothing from a project without one", () => {
    blockFor("p1").querySelector("[data-project-open]").click();
    expect(location.hash).toBe("#/project/p1/branch/main/changes");
    expect(blockFor("p2").querySelector("[data-project-open]").classList.contains("inbox-unroutable")).toBe(true);
    blockFor("p2").querySelector("[data-project-open]").click();
    expect(location.hash).toBe("#/project/p1/branch/main/changes");
  });

  it("folds a block shut by its chevron, keeps it shut across the feed and a reload, and opens it again", async () => {
    blockFor("p1").querySelector("[data-project-fold]").click();
    expect(blockFor("p1").classList.contains("inbox-folded")).toBe(true);
    expect(blockFor("p1").querySelector("[data-project-fold]").getAttribute("aria-expanded")).toBe("false");
    expect(JSON.parse(localStorage.getItem("build.inbox.folded"))).toEqual(["p1"]);
    feed(feedItems);
    expect(blockFor("p1").classList.contains("inbox-folded")).toBe(true);

    vi.resetModules();
    ({ initInboxRail } = await import("../src/core/inboxShell.js"));
    document.body.innerHTML = bodyHtml;
    initInboxRail();
    expect(blockFor("p1").classList.contains("inbox-folded")).toBe(true);

    blockFor("p1").querySelector("[data-project-fold]").click();
    expect(blockFor("p1").classList.contains("inbox-folded")).toBe(false);
    expect(JSON.parse(localStorage.getItem("build.inbox.folded"))).toEqual([]);
  });

  it("opens the create surface on the block's project from its +, on the Branch tab", () => {
    blockFor("p2").querySelector('[data-project-create="p2"]').click();
    expect(openCreateWork).toHaveBeenCalledTimes(1);
    const [options] = openCreateWork.mock.calls[0];
    expect(options).toMatchObject({ projectId: "p2", projectName: "dotfiles", kind: "branch" });
    expect(typeof options.navigate).toBe("function");
  });

  it("opens the new-repository sheet from the top, and re-reads the feed once it is made", () => {
    list().querySelector("[data-new-project]").click();
    expect(openNewRepo).toHaveBeenCalledTimes(1);
    openNewRepo.mock.calls[0][0]();
    expect(refreshFeed).toHaveBeenCalled();
  });

  it("gives each block its own Recent, opened and shut on its own", async () => {
    const live = Array.from({ length: 5 }, (_, index) =>
      branchRow({ branch: `build/live-${index}`, run_id: `run-live-${index}`, anchor: hoursAgo(index + 1) }),
    );
    feed([
      ...live,
      branchRow({ branch: "build/old", run_id: "run-old", anchor: hoursAgo(300), last_activity: hoursAgo(40) }),
      issueRow(),
      issueRow({ issue_id: "iss-old", anchor: hoursAgo(200), last_activity: hoursAgo(30) }),
    ]);
    const toggleIn = (projectId) => blockFor(projectId).querySelector("[data-recent-toggle]");
    // relaydb has five live rows, so its Recent stays shut; dotfiles has one, so its opens itself.
    expect(toggleIn("p1").getAttribute("aria-expanded")).toBe("false");
    expect(rowFor("run-old")).toBeNull();
    expect(toggleIn("p2").getAttribute("aria-expanded")).toBe("true");
    expect(rowFor("iss-old")).toBeTruthy();
    expect(rowFor("iss-old").parentElement.className).toBe("inbox-recent");
    expect(blockFor("p2").contains(rowFor("iss-old"))).toBe(true);

    toggleIn("p1").click();
    await flush();
    expect(rowFor("run-old")).toBeTruthy();
    expect(toggleIn("p2").getAttribute("aria-expanded")).toBe("true");
    feed(feedItems);
    expect(toggleIn("p1").getAttribute("aria-expanded")).toBe("true");
  });

  it("touches nothing when the feed repeats what it already said", () => {
    expect(churn(list(), () => feed(feedItems))).toEqual([]);
  });

  it("redraws only the row that changed, and keeps every block's and row's element", () => {
    const block = blockFor("p1");
    const branch = rowFor("run-1");
    const issue = rowFor("iss-1");
    const records = churn(list(), () => feed([branchRow({ unread_count: 4 }), primaryRow(), issueRow(), captureRow()]));
    expect(blockFor("p1")).toBe(block);
    expect(rowFor("run-1")).toBe(branch);
    expect(rowFor("iss-1")).toBe(issue);
    expect(records.length).toBeGreaterThan(0);
    expect(records.every((record) => block.contains(record.target))).toBe(true);
    expect(branch.querySelector(".inbox-unread").textContent).toBe("4");
  });

  it("keeps the blocks already there when a project moves to the front", () => {
    const relaydb = blockFor("p1");
    const dotfiles = blockFor("p2");
    feed([branchRow(), primaryRow(), issueRow({ anchor: hoursAgo(400) }), captureRow()]);
    expect(blocks().map((block) => block.dataset.project)).toEqual(["p2", "p1", "p3"]);
    expect(blockFor("p1")).toBe(relaydb);
    expect(blockFor("p2")).toBe(dotfiles);
  });
});
