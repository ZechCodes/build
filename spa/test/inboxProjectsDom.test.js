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
const homeProjects = () => [
  { id: "p1", deviceId: "dev-1", projectKey: "dev-1/p1", name: "relaydb" },
  { id: "p2", deviceId: "dev-1", projectKey: "dev-1/p2", name: "dotfiles" },
  { id: "p3", deviceId: "dev-1", projectKey: "dev-1/p3", name: "mascot" },
];
let feedProjects = homeProjects();
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
  deliverFeed: () => subscriber && subscriber({ items: feedItems, projects: feedProjects }),
  dropFeedDevice: () => {},
  primaryRunIdFor: () => null,
}));
// The bridge on each device the rail works: the home one a route that names no
// device is about, and the other machine on the account.
const homeCall = vi.fn(async () => ({ ok: true }));
const awayCall = vi.fn(async () => ({ ok: true }));
const openCreateWork = vi.fn();
vi.mock("../src/core/createWork.js", () => ({ openCreateWork: (...args) => openCreateWork(...args) }));
const openNewRepo = vi.fn();
vi.mock("../src/sheets/newRepo.js", () => ({ openNewRepo: (...args) => openNewRepo(...args) }));
const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args), notifySuccess: () => {} }));

let App;
let adoptDeviceSession;
let setContextOffline;
let initInboxRail;
let setInboxView;
let rememberDeviceFilter;

const flush = () => new Promise((done) => setTimeout(done, 0));
const list = () => document.getElementById("inbox-list");
const blocks = () => [...document.querySelectorAll("#inbox-list .inbox-project")];
const blockFor = (projectKey) => document.querySelector(`.inbox-project[data-project="${projectKey}"]`);
const rowsIn = (block) => [...block.querySelectorAll(".inbox-project-rows > .inbox-entry")].map((row) => row.dataset.key);
const rowFor = (entityId) => document.querySelector(`.inbox-entry[data-entity="${entityId}"]`);
const viewButton = (view) => document.querySelector(`[data-inbox-view="${view}"]`);

const now = () => new Date().toISOString();
const hoursAgo = (hours) => new Date(Date.now() - hours * 3600 * 1000).toISOString();

const branchRow = (over = {}) => ({
  kind: "branch",
  deviceId: "dev-1",
  project_id: "p1",
  projectKey: "dev-1/p1",
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
  deviceId: "dev-1",
  project_id: "p1",
  projectKey: "dev-1/p1",
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
  deviceId: "dev-1",
  project_id: "p2",
  projectKey: "dev-1/p2",
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
  deviceId: "dev-1",
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
  ({ adoptDeviceSession, setContextOffline } = await import("../src/core/deviceContexts.js"));
  ({ initInboxRail } = await import("../src/core/inboxShell.js"));
  ({ setInboxView } = await import("../src/core/inboxView.js"));
  ({ rememberDeviceFilter } = await import("../src/core/deviceFilter.js"));
  document.body.innerHTML = bodyHtml;
  document.body.className = "";
  localStorage.clear();
  location.hash = "";
  window.innerWidth = 1200;
  App.route = { name: "inbox" };
  App.gated = false;
  App.devices = [
    { id: "dev-1", name: "workshop", status: "online" },
    { id: "dev-2", name: "laptop", status: "online" },
  ];
  // Every row in the rail names the device it came from; the home device is
  // what a route that names none is about.
  adoptDeviceSession({ deviceId: "dev-1", call: homeCall });
  // The other device on the account answers for its own rows: the rail can only
  // work a device it holds a session for.
  adoptDeviceSession({ deviceId: "dev-2", call: awayCall });
  homeCall.mockClear();
  awayCall.mockClear();
  refreshFeed.mockClear();
  openCreateWork.mockClear();
  openNewRepo.mockClear();
  notifyError.mockClear();
  feedProjects = homeProjects();
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
    expect(blocks().map((block) => block.dataset.project)).toEqual(["dev-1/p1", "dev-1/p2", "dev-1/p3"]);
    expect(rowsIn(blockFor("dev-1/p1"))).toEqual(["branch:dev-1/p1:main", "run-1"]);
    expect(rowsIn(blockFor("dev-1/p2"))).toEqual(["iss-1"]);
    expect(rowsIn(blockFor("dev-1/p3"))).toEqual([]);
    // Nothing in mascot at all: flat, no empty line, and nothing to fold.
    expect(blockFor("dev-1/p3").classList.contains("inbox-flat")).toBe(true);
    expect(blockFor("dev-1/p3").querySelector(".inbox-project-rows").children.length).toBe(0);
    expect(blockFor("dev-1/p3").querySelector("[data-project-fold]").disabled).toBe(true);
    expect(blockFor("dev-1/p1").classList.contains("inbox-flat")).toBe(false);
    expect(blockFor("dev-1/p1").querySelector(".inbox-project-name").textContent).toBe("relaydb");
    expect(blockFor("dev-1/p1").querySelector(".inbox-project-head .inbox-unread").textContent).toBe("1");
    // The block already says which project, so the row does not.
    expect(rowFor("run-1").querySelector(".inbox-tag")).toBeNull();
    // New project heads the list.
    expect(list().firstElementChild.matches("[data-new-project]")).toBe(true);
  });

  it("stands a project whose rows have all gone quiet after the ones with live work", () => {
    feed([
      branchRow({ anchor: hoursAgo(300), last_activity: hoursAgo(40) }),
      issueRow(),
      branchRow({ project_id: "p3", projectKey: "dev-1/p3", project: "mascot", branch: "build/model", run_id: "run-3", anchor: hoursAgo(2) }),
      captureRow(),
    ]);
    expect(blocks().map((block) => block.dataset.project)).toEqual(["dev-1/p3", "dev-1/p2", "dev-1/p1"]);
    // Its quiet rows stand straight under the head as one-line rows, with the
    // block's own chevron as their fold and no Recent disclosure.
    expect(blockFor("dev-1/p1").classList.contains("inbox-flat")).toBe(true);
    expect(rowsIn(blockFor("dev-1/p1"))).toEqual(["run-1"]);
    expect(rowFor("run-1").classList.contains("inbox-quiet")).toBe(true);
    expect(rowFor("run-1").querySelector(".sdot")).toBeNull();
    expect(blockFor("dev-1/p1").querySelector("[data-recent-toggle]")).toBeNull();
    expect(blockFor("dev-1/p1").querySelector("[data-project-fold]").disabled).toBe(false);
    // Quiet rows start hidden: the block starts folded, and the chevron opens it.
    expect(blockFor("dev-1/p1").classList.contains("inbox-folded")).toBe(true);
    blockFor("dev-1/p1").querySelector("[data-project-fold]").click();
    expect(blockFor("dev-1/p1").classList.contains("inbox-folded")).toBe(false);
    expect(JSON.parse(localStorage.getItem("build.inbox.folded"))).toEqual({ "dev-1/p1": false });
  });

  it("highlights the block holding the branch or issue the route stands on", () => {
    expect(list().querySelector(".inbox-project.active")).toBeNull();
    App.route = { name: "branch", deviceId: "dev-1", projectId: "p2", branch: "x", tab: "changes" };
    feed(feedItems);
    expect([...list().querySelectorAll(".inbox-project.active")].map((block) => block.dataset.project)).toEqual(["dev-1/p2"]);
    App.route = { name: "capture", id: "cap-1" };
    feed(feedItems);
    expect(list().querySelector(".inbox-project.active")).toBeNull();
  });

  it("draws the folds and the Recent toggles as chevron icons", () => {
    expect(blockFor("dev-1/p1").querySelector("[data-project-fold] svg")).toBeTruthy();
    feed([branchRow({ branch: "build/old", run_id: "run-old", anchor: hoursAgo(300), last_activity: hoursAgo(40) }), primaryRow()]);
    expect(blockFor("dev-1/p1").querySelector("[data-recent-toggle] svg")).toBeTruthy();
  });

  it("opens a row like the inbox does", async () => {
    rowFor("run-1").click();
    await flush();
    expect(homeCall).toHaveBeenCalledWith("entity.seen", { entity_id: "run-1" });
    expect(location.hash).toBe("#/device/dev-1/project/p1/branch/build%2Flogin/changes");
  });

  it("opens the project's checkout from its name, and nothing from a project without one", () => {
    blockFor("dev-1/p1").querySelector("[data-project-open]").click();
    expect(location.hash).toBe("#/device/dev-1/project/p1/branch/main/changes");
    expect(blockFor("dev-1/p2").querySelector("[data-project-open]").classList.contains("inbox-unroutable")).toBe(true);
    blockFor("dev-1/p2").querySelector("[data-project-open]").click();
    expect(location.hash).toBe("#/device/dev-1/project/p1/branch/main/changes");
  });

  it("folds a block shut by its chevron, keeps it shut across the feed and a reload, and opens it again", async () => {
    blockFor("dev-1/p1").querySelector("[data-project-fold]").click();
    expect(blockFor("dev-1/p1").classList.contains("inbox-folded")).toBe(true);
    expect(blockFor("dev-1/p1").querySelector("[data-project-fold]").getAttribute("aria-expanded")).toBe("false");
    expect(JSON.parse(localStorage.getItem("build.inbox.folded"))).toEqual({ "dev-1/p1": true });
    feed(feedItems);
    expect(blockFor("dev-1/p1").classList.contains("inbox-folded")).toBe(true);

    vi.resetModules();
    ({ initInboxRail } = await import("../src/core/inboxShell.js"));
    document.body.innerHTML = bodyHtml;
    initInboxRail();
    expect(blockFor("dev-1/p1").classList.contains("inbox-folded")).toBe(true);

    blockFor("dev-1/p1").querySelector("[data-project-fold]").click();
    expect(blockFor("dev-1/p1").classList.contains("inbox-folded")).toBe(false);
    expect(JSON.parse(localStorage.getItem("build.inbox.folded"))).toEqual({ "dev-1/p1": false });
  });

  it("opens the create surface on the block's project from its +, on the Branch tab", () => {
    blockFor("dev-1/p2").querySelector('[data-project-create="dev-1/p2"]').click();
    expect(openCreateWork).toHaveBeenCalledTimes(1);
    const [options] = openCreateWork.mock.calls[0];
    expect(options).toMatchObject({ projectId: "p2", deviceId: "dev-1", projectName: "dotfiles", kind: "branch" });
    expect(typeof options.navigate).toBe("function");
  });

  it("expands a collapsed project when creating a branch in it", () => {
    blockFor("dev-1/p1").querySelector("[data-project-fold]").click();
    expect(blockFor("dev-1/p1").classList.contains("inbox-folded")).toBe(true);

    blockFor("dev-1/p1").querySelector('[data-project-create="dev-1/p1"]').click();

    expect(blockFor("dev-1/p1").classList.contains("inbox-folded")).toBe(false);
    expect(blockFor("dev-1/p1").querySelector("[data-project-fold]").getAttribute("aria-expanded")).toBe("true");
    expect(JSON.parse(localStorage.getItem("build.inbox.folded"))).toEqual({ "dev-1/p1": false });
    expect(openCreateWork).toHaveBeenCalledTimes(1);
  });

  it("expands a quiet project that started collapsed when creating a branch in it", () => {
    feed([branchRow({ anchor: hoursAgo(300), last_activity: hoursAgo(40) })]);
    expect(blockFor("dev-1/p1").classList.contains("inbox-folded")).toBe(true);

    blockFor("dev-1/p1").querySelector('[data-project-create="dev-1/p1"]').click();

    expect(blockFor("dev-1/p1").classList.contains("inbox-folded")).toBe(false);
    expect(JSON.parse(localStorage.getItem("build.inbox.folded"))).toEqual({ "dev-1/p1": false });
  });

  it("opens the new-repository sheet from the top, on the machine creation goes to", () => {
    list().querySelector("[data-new-project]").click();
    expect(openNewRepo).toHaveBeenCalledTimes(1);
    // The sheet is handed the creation device's own connection and its name;
    // it asks nothing about devices itself.
    expect(openNewRepo.mock.calls[0][1]).toEqual({ callRpc: homeCall, deviceName: "workshop" });
    openNewRepo.mock.calls[0][0]();
    expect(refreshFeed).toHaveBeenCalled();
  });

  it("refuses to add a project while no device can answer", async () => {
    const { allDevicesOfflineText } = await import("../src/core/text.js");
    setContextOffline("dev-1", { offline: true });
    setContextOffline("dev-2", { offline: true });

    list().querySelector("[data-new-project]").click();

    expect(openNewRepo).not.toHaveBeenCalled();
    expect(notifyError).toHaveBeenCalledWith("No device can take a new project", allDevicesOfflineText());
  });

  it("gives each block its own Recent, opened and shut on its own", async () => {
    const live = Array.from({ length: 5 }, (_, index) =>
      branchRow({ branch: `build/live-${index}`, run_id: `run-live-${index}`, anchor: hoursAgo(index + 1) }),
    );
    feed([
      ...live,
      branchRow({ branch: "build/old", run_id: "run-old", anchor: hoursAgo(300), last_activity: hoursAgo(40) }),
      issueRow(),
      issueRow({ issue_id: "iss-old", anchor: hoursAgo(200), last_activity: hoursAgo(30), working: false }),
    ]);
    const toggleIn = (projectKey) => blockFor(projectKey).querySelector("[data-recent-toggle]");
    // Both start shut, however thin the block above; each opens on its own press.
    expect(toggleIn("dev-1/p1").getAttribute("aria-expanded")).toBe("false");
    expect(rowFor("run-old")).toBeNull();
    expect(toggleIn("dev-1/p2").getAttribute("aria-expanded")).toBe("false");
    expect(rowFor("iss-old")).toBeNull();
    toggleIn("dev-1/p2").click();
    await flush();
    expect(toggleIn("dev-1/p2").getAttribute("aria-expanded")).toBe("true");
    expect(rowFor("iss-old")).toBeTruthy();
    expect(rowFor("iss-old").parentElement.className).toBe("inbox-recent");
    expect(blockFor("dev-1/p2").contains(rowFor("iss-old"))).toBe(true);
    // Recent's rows are quiet rows: one line, no dot.
    expect(rowFor("iss-old").classList.contains("inbox-quiet")).toBe(true);
    expect(rowFor("iss-old").querySelector(".sdot")).toBeNull();

    toggleIn("dev-1/p1").click();
    await flush();
    expect(rowFor("run-old")).toBeTruthy();
    expect(toggleIn("dev-1/p2").getAttribute("aria-expanded")).toBe("true");
    feed(feedItems);
    expect(toggleIn("dev-1/p1").getAttribute("aria-expanded")).toBe("true");
  });

  it("moves a cleared row into its project's accessible Recent fold immediately", async () => {
    feed([branchRow({ unread: false, unread_count: 0 }), issueRow()]);
    const row = rowFor("run-1");
    row.querySelector("[data-menu]").click();
    row.querySelector("[data-dismiss]").click();

    const relaydb = blockFor("dev-1/p1");
    expect(relaydb.classList.contains("inbox-flat")).toBe(true);
    expect(relaydb.classList.contains("inbox-folded")).toBe(true);
    expect(rowFor("run-1").classList.contains("inbox-quiet")).toBe(true);

    relaydb.querySelector("[data-project-fold]").click();
    expect(relaydb.classList.contains("inbox-folded")).toBe(false);
    expect(rowFor("run-1")).toBeTruthy();
    await flush();
    expect(homeCall).toHaveBeenCalledWith("entity.dismiss", { entity_id: "run-1" });
  });

  // Every device mints its project ids from its own counter, so both machines
  // have a `proj-1`. Two blocks, two selectors, and the device said out loud
  // only when the project's name alone does not say which is which.
  it("two devices each with proj-1 render two blocks with distinct keys", () => {
    feedProjects.push({ id: "p1", deviceId: "dev-2", projectKey: "dev-2/p1", name: "relaydb" });
    feed([
      branchRow(),
      branchRow({ deviceId: "dev-2", projectKey: "dev-2/p1", branch: "build/far", run_id: "run-far" }),
    ]);
    expect(blocks().map((block) => block.dataset.project)).toContain("dev-1/p1");
    expect(blocks().map((block) => block.dataset.project)).toContain("dev-2/p1");
    expect(rowsIn(blockFor("dev-1/p1"))).toEqual(["run-1"]);
    expect(rowsIn(blockFor("dev-2/p1"))).toEqual(["run-far"]);
    // Each block folds on its own, under its own name.
    blockFor("dev-2/p1").querySelector("[data-project-fold]").click();
    expect(blockFor("dev-2/p1").classList.contains("inbox-folded")).toBe(true);
    expect(blockFor("dev-1/p1").classList.contains("inbox-folded")).toBe(false);
    expect(JSON.parse(localStorage.getItem("build.inbox.folded"))).toEqual({ "dev-2/p1": true });
  });

  it("device name shown only on the name clash", () => {
    expect(blockFor("dev-1/p1").querySelector(".inbox-project-name .dim")).toBeNull();

    feedProjects.push({ id: "p1", deviceId: "dev-2", projectKey: "dev-2/p1", name: "relaydb" });
    feedProjects.push({ id: "p7", deviceId: "dev-2", projectKey: "dev-2/p7", name: "notes" });
    feed(feedItems);
    expect(blockFor("dev-1/p1").querySelector(".inbox-project-name .dim").textContent).toBe("workshop");
    expect(blockFor("dev-2/p1").querySelector(".inbox-project-name .dim").textContent).toBe("laptop");
    expect(blockFor("dev-2/p1").querySelector(".inbox-project-name").textContent).toBe("relaydb laptop");
    expect(blockFor("dev-2/p7").querySelector(".inbox-project-name .dim")).toBeNull();
    expect(blockFor("dev-1/p2").querySelector(".inbox-project-name .dim")).toBeNull();
  });

  it("creates in the bare project the bridge minted, not the account's name for it", () => {
    feedProjects.push({ id: "p1", deviceId: "dev-2", projectKey: "dev-2/p1", name: "relaydb" });
    feed(feedItems);
    blockFor("dev-2/p1").querySelector("[data-project-create]").click();
    expect(openCreateWork.mock.calls[0][0]).toMatchObject({ projectId: "p1", deviceId: "dev-2", projectName: "relaydb" });
  });

  // The block of a device that cannot answer stays on the rail — its work has
  // not gone anywhere — but nothing in it can be worked until the device is
  // back, and the rail says so rather than failing on the press.
  it("greys a block whose device is offline and shuts its +", () => {
    setContextOffline("dev-2", { offline: true });
    feedProjects.push({ id: "p1", deviceId: "dev-2", projectKey: "dev-2/p1", name: "relaydb" });
    feed([branchRow(), branchRow({ deviceId: "dev-2", projectKey: "dev-2/p1", branch: "build/far", run_id: "run-far" })]);

    const away = blockFor("dev-2/p1");
    expect(away.classList.contains("inbox-offline")).toBe(true);
    const create = away.querySelector("[data-project-create]");
    expect(create.disabled).toBe(true);
    expect(create.title).toBe("Device offline");
    expect(blockFor("dev-1/p1").classList.contains("inbox-offline")).toBe(false);
    expect(blockFor("dev-1/p1").querySelector("[data-project-create]").disabled).toBe(false);

    create.click();
    expect(openCreateWork).not.toHaveBeenCalled();
  });

  // The picker narrows the face, not the app: a block that goes with the filter
  // takes nothing with it, least of all the surface the reader is standing on.
  it("the filter hides the other device's projects and never changes the route", () => {
    feedProjects.push({ id: "p1", deviceId: "dev-2", projectKey: "dev-2/p1", name: "relaydb" });
    feed([
      branchRow(),
      branchRow({ deviceId: "dev-2", projectKey: "dev-2/p1", branch: "build/far", run_id: "run-far" }),
    ]);
    App.route = { name: "branch", deviceId: "dev-2", projectId: "p1", branch: "build/far", tab: "changes" };
    location.hash = "#/device/dev-2/project/p1/branch/build%2Ffar/changes";
    const standing = App.route;

    rememberDeviceFilter("dev-1");

    expect(blocks().map((block) => block.dataset.project)).toEqual(["dev-1/p1", "dev-1/p2", "dev-1/p3"]);
    expect(blockFor("dev-2/p1")).toBeNull();
    expect(App.route).toBe(standing);
    expect(location.hash).toBe("#/device/dev-2/project/p1/branch/build%2Ffar/changes");

    rememberDeviceFilter(null);

    expect(rowsIn(blockFor("dev-2/p1"))).toEqual(["run-far"]);
  });

  it("opens the checkout of a block on another device, on that device", () => {
    feedProjects.push({ id: "p1", deviceId: "dev-2", projectKey: "dev-2/p1", name: "relaydb" });
    feed([
      primaryRow(),
      primaryRow({ deviceId: "dev-2", projectKey: "dev-2/p1", branch: "main", run_id: null, worktree_id: "wt-far" }),
    ]);

    const head = blockFor("dev-2/p1").querySelector("[data-project-open]");
    expect(head.classList.contains("inbox-unroutable")).toBe(false);
    head.click();
    expect(location.hash).toBe("#/device/dev-2/project/p1/branch/main/changes");
  });

  it("touches nothing when the feed repeats what it already said", () => {
    expect(churn(list(), () => feed(feedItems))).toEqual([]);
  });

  it("redraws only the row that changed, and keeps every block's and row's element", () => {
    const block = blockFor("dev-1/p1");
    const branch = rowFor("run-1");
    const issue = rowFor("iss-1");
    const records = churn(list(), () => feed([branchRow({ unread_count: 4 }), primaryRow(), issueRow(), captureRow()]));
    expect(blockFor("dev-1/p1")).toBe(block);
    expect(rowFor("run-1")).toBe(branch);
    expect(rowFor("iss-1")).toBe(issue);
    expect(records.length).toBeGreaterThan(0);
    expect(records.every((record) => block.contains(record.target))).toBe(true);
    expect(branch.querySelector(".inbox-unread").textContent).toBe("4");
  });

  it("keeps the blocks already there when a project moves to the front", () => {
    const relaydb = blockFor("dev-1/p1");
    const dotfiles = blockFor("dev-1/p2");
    feed([branchRow(), primaryRow(), issueRow({ anchor: hoursAgo(400) }), captureRow()]);
    expect(blocks().map((block) => block.dataset.project)).toEqual(["dev-1/p2", "dev-1/p1", "dev-1/p3"]);
    expect(blockFor("dev-1/p1")).toBe(relaydb);
    expect(blockFor("dev-1/p2")).toBe(dotfiles);
  });
});
