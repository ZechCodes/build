// The rail's projects face, as a pure model: the inbox grouped by project —
// every project a block, its rows beneath it in the inbox's own order, the
// blocks in the order their first live row holds on the inbox, a Recent fold
// per block, and the unrouted captures standing above them all.

import { describe, it, expect } from "vitest";
import {
  blockIsFolded,
  clashingProjectNames,
  deviceTagHtml,
  deviceTags,
  projectBlockHtml,
  projectBlocks,
  projectHeadHtml,
  workspaceProjectBlocks,
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

describe("workspace project blocks", () => {
  // A workspace and a project each belong to one machine, so both are grouped
  // and marked by their account-wide names, never by the bare ids a bridge
  // minted: two machines each hold a `proj-1`.
  it("groups workspaces by project key and includes empty and inferred projects", () => {
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
      key: `workspace:dev-1/${workspaceId}`,
      workspaceId,
      workspaceKey: `dev-1/${workspaceId}`,
      deviceId: "dev-1",
      projectId: "p1",
      projectKey: "dev-1/p1",
      project: "Project",
      unreadCount: 0,
      route: { name: "workspace", deviceId: "dev-1", projectId: "p1", workspaceId, tab: "changes" },
    }));
    const projects = [{ id: "p1", projectKey: "dev-1/p1", deviceId: "dev-1", name: "Project" }];
    expect(workspaceProjectBlocks(entries, projects, "dev-1/active").blocks[0].route.workspaceId).toBe("active");
  });
});
