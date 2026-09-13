// The rail's projects face, as a pure model: the inbox grouped by project —
// every project a block, its rows beneath it in the inbox's own order, the
// blocks in the order their first live row holds on the inbox, a Recent fold
// per block, and the unrouted captures standing above them all.

import { describe, it, expect } from "vitest";
import { blockIsFolded, projectBlockHtml, projectBlocks, projectHeadHtml, workspaceProjectBlocks } from "../src/core/inboxProjects.js";
import { projectRoute } from "../src/core/projectModel.js";

const NOW = Date.parse("2026-09-02T12:00:00Z");
const ago = (hours) => new Date(NOW - hours * 3600 * 1000).toISOString();

const projects = [
  { id: "p1", name: "relaydb" },
  { id: "p2", name: "dotfiles" },
  { id: "p3", name: "mascot" },
];

const branch = (over = {}) => ({
  kind: "branch",
  project_id: "p1",
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
  project_id: "p2",
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
  it("groups normalized workspaces by project identity and includes empty and inferred projects", () => {
    const entry = (id, projectId, project, unreadCount = 0) => ({
      key: `workspace:${id}`, workspaceId: id, projectId, project, unreadCount,
      route: { name: "workspace", projectId, workspaceId: id, tab: "changes" },
    });
    const { blocks, unsorted } = workspaceProjectBlocks([
      entry("one", "p1", "Same", 2),
      entry("two", "p9", "Same", 3),
      entry("loose", "", ""),
    ], [{ id: "p1", name: "Same" }, { id: "p2", name: "Empty" }]);
    expect(blocks.map((block) => block.id)).toEqual(["p1", "p2", "p9"]);
    expect(blocks.map((block) => keys(block.entries))).toEqual([["workspace:one"], [], ["workspace:two"]]);
    expect(blocks.map((block) => block.unreadCount)).toEqual([2, 0, 3]);
    expect(keys(unsorted)).toEqual(["workspace:loose"]);
    expect(projectHeadHtml(blocks[0], {})).toContain('data-project-create="p1"');
    expect(projectHeadHtml(blocks[0], {})).toContain("New workspace in Same");
  });

  it("uses the active workspace as its project's heading destination", () => {
    const entries = ["first", "active"].map((workspaceId) => ({
      key: `workspace:${workspaceId}`,
      workspaceId,
      projectId: "p1",
      project: "Project",
      unreadCount: 0,
      route: { name: "workspace", projectId: "p1", workspaceId, tab: "changes" },
    }));
    expect(workspaceProjectBlocks(entries, [{ id: "p1", name: "Project" }], "active").blocks[0].route.workspaceId).toBe("active");
  });
});

describe("the blocks the projects face lists", () => {
  it("maps project.add results to their primary surface", () => {
    expect(projectRoute({ project_id: "folder-1", is_git: false, base_branch: "trunk" })).toEqual({
      name: "branch", projectId: "folder-1", branch: "trunk", tab: "files",
    });
    expect(projectRoute({ project_id: "repo-1", is_git: true, base_branch: "main" })).toEqual({
      name: "branch", projectId: "repo-1", branch: "main", tab: "changes",
    });
  });
  it("files every row under its project in inbox order, and stands the blocks in the order their first live row holds", () => {
    const { blocks } = projectBlocks({
      projects,
      nowMs: NOW,
      items: [
        issue({ anchor: ago(2) }),
        branch({ anchor: ago(3) }),
        branch({ branch: "build/newer", run_id: "run-2", anchor: ago(1) }),
        branch({ project_id: "p3", project: "mascot", branch: "build/model", run_id: "run-3", anchor: ago(10) }),
      ],
    });
    // The inbox reads mascot (10h), relaydb (3h), dotfiles (2h), relaydb (1h):
    // the top row's project is the top block, and so on down.
    expect(names(blocks)).toEqual(["mascot", "relaydb", "dotfiles"]);
    expect(keys(blocks[1].entries)).toEqual(["run-1", "run-2"]);
    expect(keys(blocks[2].entries)).toEqual(["iss-1"]);
  });

  it("lists a project with nothing in it after the ones with work, in the device's own order", () => {
    const { blocks } = projectBlocks({ projects, nowMs: NOW, items: [issue()] });
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
      nowMs: NOW,
      items: [
        branch({ anchor: ago(300), last_activity: ago(40) }),
        issue({ anchor: ago(1) }),
        branch({ project_id: "p3", project: "mascot", branch: "build/model", run_id: "run-3", anchor: ago(2) }),
      ],
    });
    expect(names(blocks)).toEqual(["mascot", "dotfiles", "relaydb"]);
    expect(blocks[2].entries).toEqual([]);
    expect(keys(blocks[2].recent)).toEqual(["run-1"]);
  });

  it("keeps the unrouted captures out of every block — they stand on their own, oldest first", () => {
    const { unsorted, blocks } = projectBlocks({
      projects,
      nowMs: NOW,
      items: [
        capture({ capture_id: "cap-new", anchor: ago(0.2) }),
        capture({ capture_id: "cap-old", anchor: ago(0.9) }),
        capture({ capture_id: "cap-routed", project_id: "p1", project: "relaydb", state: "routed", routing: { kind: "issue" } }),
      ],
    });
    expect(keys(unsorted)).toEqual(["capture:cap-old", "capture:cap-new"]);
    expect(keys(blocks.find((block) => block.id === "p1").entries)).toEqual(["capture:cap-routed"]);
  });

  it("partitions each block's quiet rows into its own Recent", () => {
    const { blocks } = projectBlocks({
      projects,
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
      nowMs: NOW,
      items: [branch({ anchor: ago(300), last_activity: ago(40) }), issue()],
    });
    const quietOnly = blocks.find((block) => block.id === "p1");
    const live = blocks.find((block) => block.id === "p2");
    const empty = blocks.find((block) => block.id === "p3");
    expect(blockIsFolded(quietOnly, new Map())).toBe(true);
    expect(blockIsFolded(live, new Map())).toBe(false);
    expect(blockIsFolded(empty, new Map())).toBe(false);
    expect(blockIsFolded(quietOnly, new Map([["p1", false]]))).toBe(false);
    expect(blockIsFolded(live, new Map([["p2", true]]))).toBe(true);
  });

  it("drops finished and implemented rows, while filing cleared rows under Recent", () => {
    const { blocks } = projectBlocks({
      projects,
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
      nowMs: NOW,
      items: [branch(), issue({ project_id: "p9", project: "stray" })],
    });
    expect(blocks.map((block) => [block.id, block.name])).toEqual([
      ["p1", "relaydb"],
      ["p9", "stray"],
    ]);
  });

  it("opens a block on the project's primary checkout, and nowhere when the feed has none", () => {
    const { blocks } = projectBlocks({
      projects,
      nowMs: NOW,
      items: [branch({ branch: "main", run_id: null, primary: true, can_finish: false, dismissed: true }), issue()],
    });
    expect(blocks.find((block) => block.id === "p1").route).toEqual({
      name: "branch",
      projectId: "p1",
      branch: "main",
      tab: "changes",
    });
    expect(blocks.find((block) => block.id === "p2").route).toBeNull();
  });

  it("opens a plain folder directly in Files even though it has no board row", () => {
    const { blocks } = projectBlocks({
      projects: [{ id: "folder-1", name: "notes", is_git: false, base_branch: "main" }],
      items: [],
      nowMs: NOW,
    });
    expect(blocks[0].route).toEqual({
      name: "branch",
      projectId: "folder-1",
      branch: "main",
      tab: "files",
    });
    expect(projectHeadHtml(blocks[0], {})).not.toContain("data-project-create");
  });

  it("counts a block's unread across every row in it", () => {
    const { blocks } = projectBlocks({
      projects,
      nowMs: NOW,
      items: [branch({ unread: true, unread_count: 2 }), branch({ branch: "b", run_id: "run-2", unread: true, unread_count: 3 }), issue()],
    });
    expect(blocks.find((block) => block.id === "p1").unreadCount).toBe(5);
    expect(blocks.find((block) => block.id === "p2").unreadCount).toBe(0);
  });
});

describe("what a block looks like", () => {
  const block = () => projectBlocks({ projects, nowMs: NOW, items: [branch({ unread: true, unread_count: 2 })] }).blocks[0];

  it("heads the legacy block without branch or issue creation controls", () => {
    const html = projectHeadHtml(block(), {});
    expect(html).toContain('data-project-fold="p1"');
    expect(html).toContain('aria-expanded="true"');
    expect(html).toContain('data-project-open="p1"');
    expect(html).toContain(">relaydb<");
    expect(html).toContain('class="badge inbox-unread">2<');
    expect(html).not.toContain("data-project-create");
    expect(html).not.toMatch(/branch|issue/i);
    expect(html).not.toContain("splitbtn");
    expect(html).not.toContain("data-menu=");
  });

  it("draws the fold as a chevron icon, down when open and right when the block is folded", () => {
    expect(projectHeadHtml(block(), {})).toMatch(/data-project-fold="p1"[^>]*>[\s\S]*?<svg[^>]*lucide-chevron-down/);
    const folded = projectHeadHtml(block(), { folded: new Set(["p1"]) });
    expect(folded).toContain('aria-expanded="false"');
    expect(folded).toMatch(/data-project-fold="p1"[^>]*>[\s\S]*?<svg[^>]*lucide-chevron-right/);
  });

  it("marks a block with no checkout to open as unroutable and says so", () => {
    const { blocks } = projectBlocks({ projects: [projects[1]], nowMs: NOW, items: [] });
    const html = projectHeadHtml(blocks[0], {});
    expect(html).toContain("inbox-unroutable");
    expect(html).not.toContain('class="badge');
  });

  it("escapes the project's name", () => {
    const { blocks } = projectBlocks({ projects: [{ id: "px", name: "<b>x</b>" }], nowMs: NOW, items: [] });
    const html = projectHeadHtml(blocks[0], {});
    expect(html).not.toContain("<b>x</b>");
    expect(html).toContain("&lt;b&gt;x&lt;/b&gt;");
  });

  it("wraps the head and an empty rows container, keyed by the project, folded when told", () => {
    const html = projectBlockHtml(block(), { folded: new Set(["p1"]) });
    expect(html).toContain('data-key="project:p1"');
    expect(html).toContain("inbox-folded");
    expect(html).toMatch(/<div class="inbox-project-rows"><\/div>/);
    expect(projectBlockHtml(block(), {})).not.toContain("inbox-folded");
  });

  it("lays a block with nothing live flat, and disables its fold when there is nothing to fold", () => {
    const { blocks } = projectBlocks({
      projects,
      nowMs: NOW,
      items: [branch({ anchor: ago(300), last_activity: ago(40) })],
    });
    const quietOnly = blocks.find((block) => block.id === "p1");
    const empty = blocks.find((block) => block.id === "p2");
    expect(projectBlockHtml(quietOnly, {})).toMatch(/class="inbox-project inbox-flat"/);
    expect(projectHeadHtml(quietOnly, {})).not.toContain("disabled");
    expect(projectBlockHtml(empty, {})).toMatch(/class="inbox-project inbox-flat"/);
    expect(projectHeadHtml(empty, {})).toMatch(/data-project-fold="p2"[^>]*disabled/);
    expect(projectBlockHtml(block(), {})).not.toContain("inbox-flat");
  });

  it("marks the block the route stands in as active, and no other", () => {
    expect(projectBlockHtml(block(), { activeProjectId: "p1" })).toMatch(/class="inbox-project active"/);
    expect(projectBlockHtml(block(), { activeProjectId: "p2" })).not.toContain(" active");
  });

});
