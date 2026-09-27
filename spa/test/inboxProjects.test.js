// The rail's projects face, as a pure model: the workspaces grouped by the
// project they are in — every project a block, keyed by the account-wide
// project key, with the machine said after a name two machines share.

import { describe, it, expect } from "vitest";
import { deviceTagHtml, deviceTags, projectHeadHtml, workspaceProjectBlocks } from "../src/core/inboxProjects.js";

const devices = [
  { id: "dev-1", name: "workshop" },
  { id: "dev-2", name: "laptop" },
];

const on = (deviceId, project) => ({ ...project, deviceId, projectKey: `${deviceId}/${project.id}` });

const keys = (entries) => entries.map((entry) => entry.key);

/** One workspace row as the rail hands it to the model. */
const entry = (id, projectId, project, unreadCount = 0, over = {}) => ({
  kind: "workspace",
  key: `workspace:dev-1/${id}`,
  workspaceId: id,
  workspaceKey: `dev-1/${id}`,
  deviceId: "dev-1",
  projectId,
  projectKey: projectId ? `dev-1/${projectId}` : "",
  project,
  unreadCount,
  route: { name: "workspace", deviceId: "dev-1", projectId, workspaceId: id, tab: "changes" },
  ...over,
});

describe("workspace project blocks", () => {
  it("orders blocks by bridge project summaries, then rows within each project", () => {
    const hour = 60 * 60 * 1000;
    const session = (start, last = start) => ({ session_started_ms: start * hour, last_activity_ms: last * hour });
    const projects = [
      { id: "p1", projectKey: "dev-1/p1", deviceId: "dev-1", name: "Zulu", ...session(10, 20) },
      { id: "p2", projectKey: "dev-1/p2", deviceId: "dev-1", name: "Alpha", ...session(25) },
    ];
    const rows = [
      entry("late", "p1", "Zulu", 0, { anchorMs: 20 * hour, lastActivityMs: 20 * hour }),
      entry("early", "p1", "Zulu", 0, { anchorMs: 10 * hour, lastActivityMs: 10 * hour }),
      entry("other", "p2", "Alpha", 0, { anchorMs: 25 * hour, lastActivityMs: 25 * hour }),
    ];
    const { blocks } = workspaceProjectBlocks(rows, projects, [], null, 30 * hour);
    expect(blocks.map((block) => block.projectKey)).toEqual(["dev-1/p1", "dev-1/p2"]);
    expect(blocks[0].entries.map((row) => row.workspaceId)).toEqual(["early", "late"]);
    expect(blocks[0].anchorMs).toBe(10 * hour);
  });

  it("puts an aged project block in Recent while a fresh project agent keeps another live", () => {
    const hour = 60 * 60 * 1000;
    const session = (time) => ({ session_started_ms: time * hour, last_activity_ms: time * hour });
    const projects = [
      { id: "p1", projectKey: "dev-1/p1", deviceId: "dev-1", name: "Fresh", ...session(40) },
      { id: "p2", projectKey: "dev-1/p2", deviceId: "dev-1", name: "Old", ...session(20) },
    ];
    const rows = [
      entry("old-in-fresh", "p1", "Fresh", 0, { anchorMs: 10 * hour, lastActivityMs: 10 * hour }),
      entry("old", "p2", "Old", 0, { anchorMs: 20 * hour, lastActivityMs: 20 * hour }),
    ];
    const { blocks, recentBlocks } = workspaceProjectBlocks(rows, projects, [], null, 50 * hour);
    expect(blocks.map((block) => block.projectKey)).toEqual(["dev-1/p1"]);
    expect(blocks[0].recent.map((row) => row.workspaceId)).toEqual(["old-in-fresh"]);
    expect(recentBlocks.map((block) => block.projectKey)).toEqual(["dev-1/p2"]);
  });

  it("uses finished-workspace activity in the bridge project summary", () => {
    const recent = 200;
    const project = { id: "p1", projectKey: "dev-1/p1", deviceId: "dev-1", name: "Done work",
      session_started_ms: recent, last_activity_ms: recent };
    // The bridge counted a finished workspace, which no longer has an inbox
    // row. The project summary alone keeps its block in the active section.
    const visibleRows = [];
    const { blocks, recentBlocks } = workspaceProjectBlocks(visibleRows, [project], [], null, recent + 24 * 60 * 60 * 1000);
    expect(blocks[0].anchorMs).toBe(recent);
    expect(recentBlocks).toEqual([]);
    const aged = workspaceProjectBlocks(visibleRows, [project], [], null, recent + 24 * 60 * 60 * 1000 + 1);
    expect(aged.recentBlocks[0].anchorMs).toBe(recent);
  });

  // A workspace and a project each belong to one machine, so both are grouped
  // and marked by their account-wide names, never by the bare ids a bridge
  // minted: two machines each hold a `proj-1`.
  it("groups workspaces by project key and includes empty and inferred projects", () => {
    const { blocks, unsorted } = workspaceProjectBlocks(
      [entry("one", "p1", "Same", 2), entry("two", "p9", "Same", 3), entry("loose", "", "")],
      [
        { id: "p1", projectKey: "dev-1/p1", deviceId: "dev-1", name: "Same" },
        { id: "p2", projectKey: "dev-1/p2", deviceId: "dev-1", name: "Empty" },
      ],
      [{ id: "dev-1", name: "Laptop" }],
    );
    expect(blocks.map((block) => block.projectKey)).toEqual(["dev-1/p2", "dev-1/p1", "dev-1/p9"]);
    expect(blocks.map((block) => keys(block.entries))).toEqual([[], ["workspace:dev-1/one"], ["workspace:dev-1/two"]]);
    expect(blocks.map((block) => block.unreadCount)).toEqual([0, 2, 3]);
    expect(keys(unsorted)).toEqual(["workspace:dev-1/loose"]);
    expect(projectHeadHtml(blocks[1], {})).toContain('data-project-create="dev-1/p1"');
    expect(projectHeadHtml(blocks[1], {})).toContain("New workspace in Same");
  });

  // The head is the project, so it opens the project's own page — whatever is
  // inside the block, and whether or not anything is. Every block is routable.
  it("opens its project's own page from the head, however empty the block", () => {
    const projects = [
      { id: "p1", projectKey: "dev-1/p1", deviceId: "dev-1", name: "Project" },
      { id: "p2", projectKey: "dev-1/p2", deviceId: "dev-1", name: "Empty" },
    ];
    const { blocks } = workspaceProjectBlocks([entry("one", "p1", "Project")], projects);
    expect(blocks.map((block) => block.route)).toEqual([
      { name: "project", projectId: "p2", deviceId: "dev-1" },
      { name: "project", projectId: "p1", deviceId: "dev-1" },
    ]);
    const head = projectHeadHtml(blocks[0], {});
    expect(head).not.toContain("inbox-unroutable");
    expect(head).toContain("Open Empty");
  });
});

// Which machine a project is on is worth saying only where the name does not
// say which project it is. One account, one rule: the whole set is asked once,
// and every project reads its own answer out of that.
describe("the machine said after a project name", () => {
  const tagFor = (projects, list = devices) => (project) =>
    deviceTagHtml({ ...project, ...deviceTags(projects, list).get(project.projectKey) });

  it("says the machine on a name two machines share", () => {
    const projects = [on("dev-1", { id: "p1", name: "relaydb" }), on("dev-2", { id: "p7", name: "relaydb" })];
    const tag = tagFor(projects);
    expect(tag(projects[0])).toContain("workshop");
    expect(tag(projects[1])).toContain("laptop");
  });

  it("says nothing where the names differ", () => {
    const projects = [on("dev-1", { id: "p1", name: "relaydb" }), on("dev-2", { id: "p7", name: "dotfiles" })];
    const tag = tagFor(projects);
    expect(tag(projects[0])).toBe("");
    expect(tag(projects[1])).toBe("");
  });

  // A machine can answer before the account's device list has caught up with
  // it — paired in another tab, or still being fetched. There is no name to
  // say, so the clash is marked and nothing is said.
  it("says nothing for a machine the device list has not caught up with", () => {
    const projects = [on("dev-1", { id: "p1", name: "relaydb" }), on("dev-9", { id: "p7", name: "relaydb" })];
    const tags = deviceTags(projects, devices);
    expect(tags.get("dev-9/p7")).toEqual({ clash: true, deviceName: null, offline: false });
    expect(tagFor(projects)(projects[1])).toBe("");
  });
});

describe("the project header actions", () => {
  it("keeps the device and hover actions together, with unread at the far edge", () => {
    const block = {
      projectKey: "dev-1/p1",
      name: "relaydb",
      route: { name: "workspace" },
      entries: [],
      recent: [],
      unreadCount: 4,
      clash: true,
      deviceName: "workshop",
    };
    const html = projectHeadHtml(block);
    expect(html).toContain('class="inbox-project-tools"');
    expect(html).toContain('class="inbox-project-device"');
    expect(html).toContain("workshop");
    expect(html).toContain('class="iconbtn inbox-project-settings"');
    expect(html).toContain('class="iconbtn inbox-project-create"');
    expect(html.indexOf("inbox-project-actions")).toBeLessThan(html.indexOf("badge inbox-unread"));
  });

  // Which machine a block is on is a secondary tag, not a second name: it wears
  // a class of its own so it can be sized and quietened apart from the name it
  // hangs off. One mark, worn the same way on a block head and on a row.
  it("marks the machine as a tag of its own rather than plain dim text", () => {
    expect(deviceTagHtml({ clash: true, deviceName: "workshop" })).toBe(' <span class="dim inbox-device">workshop</span>');
  });
});

// A project whose machine cannot be asked anything says so — and says nothing
// else: its head offers what every head offers.
describe("a project on a machine that is away", () => {
  const block = (over = {}) => ({
    key: "project:dev-9/p1",
    projectKey: "dev-9/p1",
    name: "relaydb",
    entries: [],
    recent: [],
    unreadCount: 0,
    ...over,
  });

  // "Offline" always, not only on a name two machines share: which laptop holds
  // it stops being the useful fact the moment none of them can answer.
  it("says Offline instead of the machine's name, clash or no clash", () => {
    expect(deviceTagHtml({ clash: false, deviceName: null, offline: true })).toContain("Offline");
    expect(deviceTagHtml({ clash: true, deviceName: "workshop", offline: true })).toContain("Offline");
    expect(deviceTagHtml({ clash: true, deviceName: "workshop", offline: true })).not.toContain("workshop");
    expect(projectHeadHtml(block({ offline: true }))).toContain("Offline");
  });

  // A machine the account's device list has never heard of can answer for
  // nothing, so a project on it is offline by the same rule.
  it("counts a machine the device list does not name as offline", () => {
    const projects = [on("dev-9", { id: "p1", name: "relaydb" })];
    const tags = deviceTags(projects, devices, new Set());
    expect(tags.get("dev-9/p1")).toEqual({ clash: false, deviceName: null, offline: true });
    // …and a machine the list DOES name is offline only while it says so.
    const here = [on("dev-1", { id: "p1", name: "relaydb" })];
    expect(deviceTags(here, devices, new Set()).get("dev-1/p1").offline).toBe(false);
    expect(deviceTags(here, devices, new Set(["dev-1"])).get("dev-1/p1").offline).toBe(true);
  });

  // Nobody asked about machines — the toolbar's menu rows — is not the same as
  // "every machine is fine": the answer is simply not offered.
  it("says nothing about being away when the caller did not ask", () => {
    expect(deviceTags([on("dev-9", { id: "p1", name: "relaydb" })], devices).get("dev-9/p1").offline).toBe(false);
  });

  // Which controls a head has is what the cache holds, never whether its
  // machine is answering: Hide lives in the block's menu on every block.
  it("offers Hide in the block's menu, whether or not its machine is away", () => {
    const open = { openMenuKey: "project:dev-9/p1" };
    for (const offline of [true, false]) {
      const head = projectHeadHtml(block({ offline }), open);
      expect(head).toContain('data-project-hide="dev-9/p1"');
      expect(head).toContain("Hide project");
      // Behind the block's ⋯, which stands in the hover reveal ahead of the cog
      // and the +; the menu hangs off the head, after everything on it.
      expect(head).toContain('data-menu="project:dev-9/p1"');
      expect(head.indexOf("inbox-project-actions")).toBeLessThan(head.indexOf("inbox-more"));
      expect(head.indexOf("inbox-more")).toBeLessThan(head.indexOf("inbox-project-settings"));
      expect(head.indexOf("inbox-project-create")).toBeLessThan(head.indexOf("inbox-menu"));
    }
    // A menu that is shut is not in the markup at all, on any block.
    expect(projectHeadHtml(block({ offline: true }))).not.toContain("data-project-hide");
    expect(projectHeadHtml(block({ offline: true }))).toContain('data-menu="project:dev-9/p1"');
    expect(projectHeadHtml(block({ offline: true }), { openMenuKey: "workspace:dev-9/w1" })).not.toContain("data-project-hide");
  });

  it("paints the same controls on a head whose machine is away as on one that answers", () => {
    const controls = (offline) =>
      projectHeadHtml(block({ offline }), { openMenuKey: "project:dev-9/p1" })
        .match(/data-project-[a-z]+|data-menu|disabled/g);
    expect(controls(true)).toEqual(controls(false));
  });
});
