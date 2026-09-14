// The rail's projects face, as a pure model: the inbox grouped by project —
// every project a block, its rows beneath it in the inbox's own order, the
// blocks in the order their first live row holds on the inbox, a Recent fold
// per block, and the unrouted captures standing above them all.

import { describe, it, expect } from "vitest";
import {
  blockIsFolded,
  clashingProjectNames,
  deviceTagHtml,
  newProjectButtonHtml,
  projectBlockHtml,
  projectBlocks,
  projectHeadHtml,
} from "../src/core/inboxProjects.js";
import { projectRoute } from "../src/core/projectModel.js";

const NOW = Date.parse("2026-09-02T12:00:00Z");
const ago = (hours) => new Date(NOW - hours * 3600 * 1000).toISOString();

const devices = [
  { id: "dev-1", name: "workshop" },
  { id: "dev-2", name: "laptop" },
];

const on = (deviceId, project) => ({ ...project, deviceId, projectKey: `${deviceId}/${project.id}` });

const projects = [
  on("dev-1", { id: "p1", name: "relaydb" }),
  on("dev-1", { id: "p2", name: "dotfiles" }),
  on("dev-1", { id: "p3", name: "mascot" }),
];

const branch = (over = {}) => ({
  kind: "branch",
  deviceId: "dev-1",
  project_id: "p1",
  projectKey: "dev-1/p1",
  project: "relaydb",
  branch: "build/login",
  title: "Fix the login flow",
  state: "building",
  unread: false,
  unread_count: 0,
  working: false,
  stat: null,
  anchor: ago(3),
  last_activity: ago(1),
  can_finish: true,
  finish: { warnings: [] },
  muted: false,
  run_id: "run-1",
  issue_id: null,
  primary: false,
  ...over,
});

const issue = (over = {}) => ({
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
  working: false,
  stat: null,
  anchor: ago(2),
  last_activity: ago(2),
  can_finish: true,
  finish: { warnings: [] },
  muted: false,
  issue_id: "iss-1",
  implementation_active: false,
  primary: false,
  ...over,
});

const capture = (over = {}) => ({
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
  created_at: ago(0.5),
  anchor: ago(0.5),
  last_activity: ago(0.5),
  unread: false,
  unread_count: 0,
  routing: null,
  question: null,
  ...over,
});

const names = (blocks) => blocks.map((block) => block.name);
const keys = (entries) => entries.map((entry) => entry.key);

describe("the blocks the projects face lists", () => {
  it("maps project.add results to their primary surface", () => {
    expect(projectRoute({ project_id: "folder-1", is_git: false, base_branch: "trunk" })).toEqual({
      name: "branch", projectId: "folder-1", branch: "trunk", tab: "files",
    });
    expect(projectRoute({ project_id: "repo-1", deviceId: "d", is_git: true, base_branch: "main" })).toEqual({
      name: "branch", deviceId: "d", projectId: "repo-1", branch: "main", tab: "changes",
    });
  });

  // A block opens the machine its project is on: the head of a block, the
  // primary checkout under it, and a plain folder's own surface all name one.
  it("block routes carry deviceId", () => {
    const { blocks } = projectBlocks({
      projects: [
        on("dev-2", { id: "p1", name: "relaydb" }),
        on("dev-2", { id: "folder-1", name: "notes", is_git: false, base_branch: "main" }),
      ],
      devices,
      nowMs: NOW,
      items: [
        branch({ deviceId: "dev-2", projectKey: "dev-2/p1", branch: "main", run_id: null, primary: true, can_finish: false }),
      ],
    });
    expect(blocks.find((block) => block.id === "p1").route).toMatchObject({ name: "branch", deviceId: "dev-2", projectId: "p1" });
    expect(blocks.find((block) => block.id === "folder-1").route).toMatchObject({
      name: "branch",
      deviceId: "dev-2",
      projectId: "folder-1",
      tab: "files",
    });
  });

  // The rail is one list across every machine, and every block on it opens.
  it("no block is unroutable for being on another device", () => {
    const { blocks } = projectBlocks({
      projects: [on("dev-1", { id: "p1", name: "relaydb" }), on("dev-2", { id: "p1", name: "relaydb" })],
      devices,
      nowMs: NOW,
      items: [
        branch({ branch: "main", run_id: null, primary: true, can_finish: false }),
        branch({ deviceId: "dev-2", projectKey: "dev-2/p1", branch: "main", run_id: "run-2", primary: true, can_finish: false }),
      ],
    });
    expect(blocks.map((block) => block.route?.deviceId)).toEqual(["dev-1", "dev-2"]);
    expect(blocks.every((block) => projectHeadHtml(block, {}).includes("inbox-unroutable"))).toBe(false);
  });
  it("files every row under its project in inbox order, and stands the blocks in the order their first live row holds", () => {
    const { blocks } = projectBlocks({
      projects,
      devices,
      nowMs: NOW,
      items: [
        issue({ anchor: ago(2) }),
        branch({ anchor: ago(3) }),
        branch({ branch: "build/newer", run_id: "run-2", anchor: ago(1) }),
        branch({ project_id: "p3", projectKey: "dev-1/p3", project: "mascot", branch: "build/model", run_id: "run-3", anchor: ago(10) }),
      ],
    });
    // The inbox reads mascot (10h), relaydb (3h), dotfiles (2h), relaydb (1h):
    // the top row's project is the top block, and so on down.
    expect(names(blocks)).toEqual(["mascot", "relaydb", "dotfiles"]);
    expect(keys(blocks[1].entries)).toEqual(["run-1", "run-2"]);
    expect(keys(blocks[2].entries)).toEqual(["iss-1"]);
  });

  it("lists a project with nothing in it after the ones with work, in the device's own order", () => {
    const { blocks } = projectBlocks({ projects, devices, nowMs: NOW, items: [issue()] });
    expect(names(blocks)).toEqual(["dotfiles", "relaydb", "mascot"]);
    expect(blocks[1].entries).toEqual([]);
    // Nothing live in either: both are flat.
    expect(blocks.map((block) => block.flat)).toEqual([false, true, true]);
  });

  // The reviewer's screenshot: a project whose only row had gone quiet stood
  // above projects with live work, because it was ranked by that row's age.
  it("stands a project whose rows have all gone quiet after every project with live work", () => {
    const { blocks } = projectBlocks({
      projects,
      devices,
      nowMs: NOW,
      items: [
        branch({ anchor: ago(300), last_activity: ago(40) }),
        issue({ anchor: ago(1) }),
        branch({ project_id: "p3", projectKey: "dev-1/p3", project: "mascot", branch: "build/model", run_id: "run-3", anchor: ago(2) }),
      ],
    });
    expect(names(blocks)).toEqual(["mascot", "dotfiles", "relaydb"]);
    expect(blocks[2].entries).toEqual([]);
    expect(keys(blocks[2].recent)).toEqual(["run-1"]);
  });

  it("keeps the unrouted captures out of every block — they stand on their own, oldest first", () => {
    const { unsorted, blocks } = projectBlocks({
      projects,
      devices,
      nowMs: NOW,
      items: [
        capture({ capture_id: "cap-new", anchor: ago(0.2) }),
        capture({ capture_id: "cap-old", anchor: ago(0.9) }),
        capture({ capture_id: "cap-routed", project_id: "p1", projectKey: "dev-1/p1", project: "relaydb", state: "routed", routing: { kind: "issue" } }),
      ],
    });
    expect(keys(unsorted)).toEqual(["capture:cap-old", "capture:cap-new"]);
    expect(keys(blocks.find((block) => block.id === "p1").entries)).toEqual(["capture:cap-routed"]);
  });

  it("partitions each block's quiet rows into its own Recent", () => {
    const { blocks } = projectBlocks({
      projects,
      devices,
      nowMs: NOW,
      items: [
        branch(),
        branch({ branch: "build/old", run_id: "run-old", anchor: ago(300), last_activity: ago(40) }),
        ...[1, 2, 3, 4, 5].map((n) => issue({ issue_id: `iss-${n}`, anchor: ago(n) })),
        issue({ issue_id: "iss-old", anchor: ago(200), last_activity: ago(30) }),
      ],
    });
    const relaydb = blocks.find((block) => block.id === "p1");
    const dotfiles = blocks.find((block) => block.id === "p2");
    expect(keys(relaydb.entries)).toEqual(["run-1"]);
    expect(keys(relaydb.recent)).toEqual(["run-old"]);
    expect(dotfiles.entries.length).toBe(5);
    expect(keys(dotfiles.recent)).toEqual(["iss-old"]);
  });

  // Quiet rows always start hidden: a block holding only quiet rows starts
  // folded, and what the user says of a block outranks that either way.
  it("folds a quiet-only block shut to begin with, and lets the user's word stand", () => {
    const { blocks } = projectBlocks({
      projects,
      devices,
      nowMs: NOW,
      items: [branch({ anchor: ago(300), last_activity: ago(40) }), issue()],
    });
    const quietOnly = blocks.find((block) => block.id === "p1");
    const live = blocks.find((block) => block.id === "p2");
    const empty = blocks.find((block) => block.id === "p3");
    expect(blockIsFolded(quietOnly, new Map())).toBe(true);
    expect(blockIsFolded(live, new Map())).toBe(false);
    expect(blockIsFolded(empty, new Map())).toBe(false);
    expect(blockIsFolded(quietOnly, new Map([["dev-1/p1", false]]))).toBe(false);
    expect(blockIsFolded(live, new Map([["dev-1/p2", true]]))).toBe(true);
  });

  it("drops finished and implemented rows, while filing cleared rows under Recent", () => {
    const { blocks } = projectBlocks({
      projects,
      devices,
      nowMs: NOW,
      items: [
        branch({ state: "merged" }),
        branch({ branch: "build/cleared", run_id: "run-c", dismissed: true }),
        issue({ implementation_active: true, implementing_branch: "build/x" }),
        issue({ issue_id: "iss-live" }),
      ],
    });
    expect(keys(blocks.find((block) => block.id === "p1").entries)).toEqual([]);
    expect(keys(blocks.find((block) => block.id === "p1").recent)).toEqual(["run-c"]);
    expect(keys(blocks.find((block) => block.id === "p2").entries)).toEqual(["iss-live"]);
  });

  it("ranks and folds a project whose only row was cleared, even if that row is working", () => {
    const { blocks } = projectBlocks({
      projects,
      devices,
      nowMs: NOW,
      items: [branch({ dismissed: true, working: true }), issue()],
    });
    expect(names(blocks)).toEqual(["dotfiles", "relaydb", "mascot"]);
    const clearedOnly = blocks.find((block) => block.id === "p1");
    expect(clearedOnly.entries).toEqual([]);
    expect(keys(clearedOnly.recent)).toEqual(["run-1"]);
    expect(blockIsFolded(clearedOnly, new Map())).toBe(true);
  });

  it("gives a row from a project the device has not listed a block of its own, named by the row", () => {
    const { blocks } = projectBlocks({
      projects: [projects[0]],
      devices,
      nowMs: NOW,
      items: [branch(), issue({ project_id: "p9", projectKey: "dev-1/p9", project: "stray" })],
    });
    expect(blocks.map((block) => [block.id, block.name])).toEqual([
      ["p1", "relaydb"],
      ["p9", "stray"],
    ]);
  });

  it("opens a block on the project's primary checkout, and nowhere when the feed has none", () => {
    const { blocks } = projectBlocks({
      projects,
      devices,
      nowMs: NOW,
      items: [branch({ branch: "main", run_id: null, primary: true, can_finish: false, dismissed: true }), issue()],
    });
    expect(blocks.find((block) => block.id === "p1").route).toEqual({
      name: "branch",
      deviceId: "dev-1",
      projectId: "p1",
      branch: "main",
      tab: "changes",
    });
    expect(blocks.find((block) => block.id === "p2").route).toBeNull();
  });

  it("opens a plain folder directly in Files even though it has no board row", () => {
    const { blocks } = projectBlocks({
      projects: [on("dev-1", { id: "folder-1", name: "notes", is_git: false, base_branch: "main" })],
      devices,
      items: [],
      nowMs: NOW,
    });
    expect(blocks[0].route).toEqual({
      name: "branch",
      deviceId: "dev-1",
      projectId: "folder-1",
      branch: "main",
      tab: "files",
    });
    expect(projectHeadHtml(blocks[0], {})).not.toContain("data-project-create");
  });

  it("counts a block's unread across every row in it", () => {
    const { blocks } = projectBlocks({
      projects,
      devices,
      nowMs: NOW,
      items: [branch({ unread: true, unread_count: 2 }), branch({ branch: "b", run_id: "run-2", unread: true, unread_count: 3 }), issue()],
    });
    expect(blocks.find((block) => block.id === "p1").unreadCount).toBe(5);
    expect(blocks.find((block) => block.id === "p2").unreadCount).toBe(0);
  });
});

// ---- the account's blocks, not one machine's ----------------------------------
// Every device mints its project ids from its own counter, so both machines
// have a `proj-1`. The block is named by the pair, the bare id is kept for the
// wire, and the device is said out loud only when the name alone is ambiguous.
describe("two devices in one rail", () => {
  const elsewhere = (over = {}) =>
    branch({ deviceId: "dev-2", project_id: "p1", projectKey: "dev-2/p1", project: "relaydb", run_id: "run-far", ...over });

  const twoDevices = (over = {}) =>
    projectBlocks({
      projects: [projects[0], on("dev-2", { id: "p1", name: "relaydb" })],
      devices,
      nowMs: NOW,
      items: [branch(), elsewhere()],
      ...over,
    });

  it("keys blocks by projectKey and keeps the bare id", () => {
    const { blocks } = twoDevices();
    expect(blocks.map((block) => block.key)).toEqual(["project:dev-1/p1", "project:dev-2/p1"]);
    expect(blocks.map((block) => block.id)).toEqual(["p1", "p1"]);
    expect(blocks.map((block) => block.projectKey)).toEqual(["dev-1/p1", "dev-2/p1"]);
    expect(blocks.map((block) => block.deviceId)).toEqual(["dev-1", "dev-2"]);
    expect(blocks.map((block) => block.deviceName)).toEqual(["workshop", "laptop"]);
  });

  it("folds two devices' proj-1 into two blocks, in device order", () => {
    const { blocks } = twoDevices();
    expect(blocks).toHaveLength(2);
    expect(keys(blocks[0].entries)).toEqual(["run-1"]);
    expect(keys(blocks[1].entries)).toEqual(["run-far"]);
  });

  it("folds each block on its own key", () => {
    const { blocks } = twoDevices();
    expect(blockIsFolded(blocks[0], new Map([["dev-2/p1", true]]))).toBe(false);
    expect(blockIsFolded(blocks[1], new Map([["dev-2/p1", true]]))).toBe(true);
  });

  it("names the device after the project only when two devices share a project name", () => {
    expect([...clashingProjectNames(projects)]).toEqual([]);
    expect([...clashingProjectNames([projects[0], on("dev-2", { id: "p1", name: "relaydb" })])]).toEqual(["relaydb"]);
    // The same project on the same device twice is one project, not a clash.
    expect([...clashingProjectNames([projects[0], projects[0]])]).toEqual([]);

    const { blocks } = twoDevices();
    expect(blocks.map((block) => block.clash)).toEqual([true, true]);
    expect(deviceTagHtml(blocks[1])).toBe(' <span class="dim">laptop</span>');
    expect(projectHeadHtml(blocks[1], {})).toContain('<span class="dim">laptop</span>');
  });

  it("says nothing about the device when the names already tell them apart", () => {
    const { blocks } = twoDevices({
      projects: [projects[0], on("dev-2", { id: "p1", name: "mascot" })],
    });
    expect(blocks.map((block) => block.clash)).toEqual([false, false]);
    expect(blocks.map((block) => deviceTagHtml(block))).toEqual(["", ""]);
  });

  it("renders a single device's head exactly as before", () => {
    const { blocks } = projectBlocks({ projects, devices, nowMs: NOW, items: [branch()] });
    const html = projectHeadHtml(blocks[0], {});
    expect(html).not.toContain("dim");
    expect(html).toContain(">relaydb</button>");
  });
});

describe("what a block looks like", () => {
  const block = () => projectBlocks({ projects, devices, nowMs: NOW, items: [branch({ unread: true, unread_count: 2 })] }).blocks[0];

  it("heads the block with the fold, the project's name that opens it, its unread, and one + that creates", () => {
    const html = projectHeadHtml(block(), {});
    expect(html).toContain('data-project-fold="dev-1/p1"');
    expect(html).toContain('aria-expanded="true"');
    expect(html).toContain('data-project-open="dev-1/p1"');
    expect(html).toContain(">relaydb<");
    expect(html).toContain('class="badge inbox-unread">2<');
    expect(html).toMatch(/<button class="iconbtn inbox-project-create"[^>]*data-project-create="dev-1\/p1"[^>]*>[\s\S]*?<svg[^>]*lucide-plus/);
    expect(html).not.toContain("splitbtn");
    expect(html).not.toContain("data-menu=");
  });

  it("draws the fold as a chevron icon, down when open and right when the block is folded", () => {
    expect(projectHeadHtml(block(), {})).toMatch(/data-project-fold="dev-1\/p1"[^>]*>[\s\S]*?<svg[^>]*lucide-chevron-down/);
    const folded = projectHeadHtml(block(), { folded: new Set(["dev-1/p1"]) });
    expect(folded).toContain('aria-expanded="false"');
    expect(folded).toMatch(/data-project-fold="dev-1\/p1"[^>]*>[\s\S]*?<svg[^>]*lucide-chevron-right/);
  });

  it("marks a block with no checkout to open as unroutable and says so", () => {
    const { blocks } = projectBlocks({ projects: [projects[1]], devices, nowMs: NOW, items: [] });
    const html = projectHeadHtml(blocks[0], {});
    expect(html).toContain("inbox-unroutable");
    expect(html).not.toContain('class="badge');
  });

  it("escapes the project's name", () => {
    const { blocks } = projectBlocks({ projects: [on("dev-1", { id: "px", name: "<b>x</b>" })], devices, nowMs: NOW, items: [] });
    const html = projectHeadHtml(blocks[0], {});
    expect(html).not.toContain("<b>x</b>");
    expect(html).toContain("&lt;b&gt;x&lt;/b&gt;");
  });

  it("wraps the head and an empty rows container, keyed by the project, folded when told", () => {
    const html = projectBlockHtml(block(), { folded: new Set(["dev-1/p1"]) });
    expect(html).toContain('data-key="project:dev-1/p1"');
    expect(html).toContain("inbox-folded");
    expect(html).toMatch(/<div class="inbox-project-rows"><\/div>/);
    expect(projectBlockHtml(block(), {})).not.toContain("inbox-folded");
  });

  it("lays a block with nothing live flat, and disables its fold when there is nothing to fold", () => {
    const { blocks } = projectBlocks({
      projects,
      devices,
      nowMs: NOW,
      items: [branch({ anchor: ago(300), last_activity: ago(40) })],
    });
    const quietOnly = blocks.find((block) => block.id === "p1");
    const empty = blocks.find((block) => block.id === "p2");
    expect(projectBlockHtml(quietOnly, {})).toMatch(/class="inbox-project inbox-flat"/);
    expect(projectHeadHtml(quietOnly, {})).not.toContain("disabled");
    expect(projectBlockHtml(empty, {})).toMatch(/class="inbox-project inbox-flat"/);
    expect(projectHeadHtml(empty, {})).toMatch(/data-project-fold="dev-1\/p2"[^>]*disabled/);
    expect(projectBlockHtml(block(), {})).not.toContain("inbox-flat");
  });

  it("marks the block the route stands in as active, and no other", () => {
    expect(projectBlockHtml(block(), { activeProjectId: "dev-1/p1" })).toMatch(/class="inbox-project active"/);
    expect(projectBlockHtml(block(), { activeProjectId: "dev-1/p2" })).not.toContain(" active");
  });

  it("offers a new project", () => {
    expect(newProjectButtonHtml()).toContain("data-new-project");
    expect(newProjectButtonHtml()).toContain("New project");
    expect(newProjectButtonHtml()).toContain("<svg");
  });
});
