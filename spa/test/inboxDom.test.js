// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
let subscriber;
const navigate = vi.fn();
const createWorkspace = vi.fn();

vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => { subscriber = fn; return () => {}; },
  refreshFeed: vi.fn(),
}));
vi.mock("../src/core/inboxShell.js", () => ({ goFromInbox: (...args) => navigate(...args) }));
vi.mock("../src/core/createWork.js", () => ({ openCreateWork: (...args) => createWorkspace(...args) }));

const projects = [{ id: "project-1", name: "Payments" }, { id: "project-2", name: "Website" }];
const workspace = (overrides = {}) => ({
  id: "workspace-1", project_id: "project-1", name: "Checkout", root: "/work/checkout", status: "active",
  work_summary: { pushes: 2, additions: 8, deletions: 3 },
  directories: [{ id: "api", source_id: "source-api", is_git: true }], ...overrides,
});
const feed = (workspaces) => subscriber({ items: [], pending: [], projects, workspaces });
const rows = () => [...document.querySelectorAll("#inbox-list .inbox-entry")];

let App;
let mountInboxList;
let inboxListRouteChanged;
let setInboxView;

beforeEach(async () => {
  vi.resetModules();
  navigate.mockReset();
  createWorkspace.mockReset();
  document.body.innerHTML = bodyHtml;
  ({ App } = await import("../src/app.js"));
  ({ mountInboxList, inboxListRouteChanged, setInboxView } = await import("../src/core/inboxView.js"));
  App.route = { name: "inbox" };
  App.call = vi.fn(async () => ({}));
  mountInboxList();
});

describe("workspace inbox", () => {
  it("paints workspace rows with project and directory context", () => {
    feed([workspace(), workspace({ id: "workspace-2", project_id: "project-2", name: "Marketing", directories: [] })]);
    expect(rows().map((row) => row.dataset.key)).toEqual(["workspace:workspace-1", "workspace:workspace-2"]);
    expect(rows()[0].textContent).toContain("Payments");
    expect(rows()[0].textContent).toContain("2 pushes · +8 −3");
    expect(rows()[1].textContent).toContain("2 pushes · +8 −3");
  });

  it("opens the canonical workspace route", () => {
    feed([workspace()]);
    rows()[0].click();
    expect(navigate).toHaveBeenCalledWith({ name: "workspace", projectId: "project-1", workspaceId: "workspace-1", sourceId: "source-api", tab: "changes" });
  });

  it("marks the workspace named by the route", () => {
    feed([workspace(), workspace({ id: "workspace-2", name: "Second" })]);
    App.route = { name: "workspace", projectId: "project-1", workspaceId: "workspace-2", sourceId: "source-api", tab: "changes" };
    inboxListRouteChanged();
    expect(rows().map((row) => row.classList.contains("active"))).toEqual([false, true]);
  });

  it("preserves row elements while applying feed updates", () => {
    feed([workspace(), workspace({ id: "workspace-2", name: "Second" })]);
    const first = rows()[0];
    feed([workspace({ name: "Checkout updated" }), workspace({ id: "workspace-2", name: "Second" })]);
    expect(rows()[0]).toBe(first);
    expect(rows()[0].textContent).toContain("Checkout updated");
  });

  it("groups workspaces under their projects in the projects face", () => {
    feed([
      workspace(),
      workspace({ id: "workspace-2", name: "Refunds" }),
      workspace({ id: "workspace-3", project_id: "project-2", name: "Marketing", directories: [] }),
    ]);
    setInboxView("projects");
    const blocks = [...document.querySelectorAll("#inbox-list .inbox-project")];
    expect(blocks.map((block) => block.dataset.project)).toEqual(["project-1", "project-2"]);
    expect(blocks.map((block) => [...block.querySelectorAll(".inbox-entry")].map((row) => row.dataset.key))).toEqual([
      ["workspace:workspace-1", "workspace:workspace-2"],
      ["workspace:workspace-3"],
    ]);
    expect(blocks.map((block) => block.querySelector(".inbox-project-name").textContent)).toEqual(["Payments", "Website"]);
    expect(document.querySelector("#inbox-new-project")).not.toBeNull();
    expect(document.querySelector("#inbox-list .inbox-new-project")).toBeNull();
    expect(document.querySelector("[data-done], [data-dismiss], [data-mute]")).toBeNull();
    expect(document.querySelectorAll("[data-project-create]")).toHaveLength(2);
    expect(document.getElementById("inbox-list").textContent).not.toMatch(/branch|issue/i);
  });

  it("folds a project and preserves workspace row identity across refreshes", () => {
    feed([workspace(), workspace({ id: "workspace-2", name: "Refunds" })]);
    setInboxView("projects");
    const first = rows()[0];
    document.querySelector('[data-project-fold="project-1"]').click();
    expect(document.querySelector('[data-project="project-1"]').classList.contains("inbox-folded")).toBe(true);
    feed([workspace({ name: "Checkout updated" }), workspace({ id: "workspace-2", name: "Refunds" })]);
    expect(rows()[0]).toBe(first);
    expect(rows()[0].textContent).toContain("Checkout updated");
    expect(document.querySelector('[data-project="project-1"]').classList.contains("inbox-folded")).toBe(true);
  });

  it("keeps the active workspace marked in its project and opens only workspace routes", () => {
    feed([workspace(), workspace({ id: "workspace-2", project_id: "project-2", name: "Marketing", directories: [] })]);
    setInboxView("projects");
    App.route = { name: "workspace", projectId: "project-2", workspaceId: "workspace-2", tab: "changes" };
    inboxListRouteChanged();
    expect(rows().map((row) => row.classList.contains("active"))).toEqual([false, true]);
    expect(document.querySelector('[data-project="project-2"]').classList.contains("active")).toBe(true);
    document.querySelector('[data-project-open="project-2"]').click();
    expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ name: "workspace", projectId: "project-2", workspaceId: "workspace-2" }));
  });

  it("renders empty projects", () => {
    feed([workspace()]);
    setInboxView("projects");
    const blocks = [...document.querySelectorAll("#inbox-list .inbox-project")];
    expect(blocks).toHaveLength(2);
    expect(blocks[1].dataset.project).toBe("project-2");
    expect(blocks[1].querySelector("[data-project-fold]").disabled).toBe(true);
  });

  it("creates a workspace in the project named by its group", () => {
    feed([workspace()]);
    setInboxView("projects");
    document.querySelector('[data-project-create="project-2"]').click();
    expect(createWorkspace).toHaveBeenCalledWith(expect.objectContaining({
      projectId: "project-2",
      projectName: "Website",
      navigate: expect.any(Function),
    }));
    expect(createWorkspace.mock.calls[0][0]).not.toHaveProperty("kind");
  });
});
