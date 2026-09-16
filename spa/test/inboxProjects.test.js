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
const entry = (id, projectId, project, unreadCount = 0) => ({
  key: `workspace:dev-1/${id}`,
  workspaceId: id,
  workspaceKey: `dev-1/${id}`,
  deviceId: "dev-1",
  projectId,
  projectKey: projectId ? `dev-1/${projectId}` : "",
  project,
  unreadCount,
  route: { name: "workspace", deviceId: "dev-1", projectId, workspaceId: id, tab: "changes" },
});

describe("workspace project blocks", () => {
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
      null,
      [{ id: "dev-1", name: "Laptop" }],
    );
    expect(blocks.map((block) => block.projectKey)).toEqual(["dev-1/p1", "dev-1/p2", "dev-1/p9"]);
    expect(blocks.map((block) => keys(block.entries))).toEqual([["workspace:dev-1/one"], [], ["workspace:dev-1/two"]]);
    expect(blocks.map((block) => block.unreadCount)).toEqual([2, 0, 3]);
    expect(keys(unsorted)).toEqual(["workspace:dev-1/loose"]);
    expect(projectHeadHtml(blocks[0], {})).toContain('data-project-create="dev-1/p1"');
    expect(projectHeadHtml(blocks[0], {})).toContain("New workspace in Same");
  });

  it("uses the active workspace as its project's heading destination", () => {
    const entries = ["first", "active"].map((workspaceId) => ({
      ...entry(workspaceId, "p1", "Project"),
      key: `workspace:dev-1/${workspaceId}`,
    }));
    const projects = [{ id: "p1", projectKey: "dev-1/p1", deviceId: "dev-1", name: "Project" }];
    expect(workspaceProjectBlocks(entries, projects, "dev-1/active").blocks[0].route.workspaceId).toBe("active");
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
    expect(tags.get("dev-9/p7")).toEqual({ clash: true, deviceName: null });
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
});
