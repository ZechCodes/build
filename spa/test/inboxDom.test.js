// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
let subscriber;
const navigate = vi.fn();

vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => { subscriber = fn; return () => {}; },
  refreshFeed: vi.fn(),
}));
vi.mock("../src/core/inboxShell.js", () => ({ goFromInbox: (...args) => navigate(...args) }));

const projects = [{ id: "project-1", name: "Payments" }, { id: "project-2", name: "Website" }];
const workspace = (overrides = {}) => ({
  id: "workspace-1", project_id: "project-1", name: "Checkout", root: "/work/checkout", status: "active",
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
    expect(rows()[0].textContent).toContain("1 directory · 1 Git");
    expect(rows()[1].textContent).toContain("No directories");
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

  it("shows workspaces in either rail face without legacy actions", () => {
    feed([workspace()]);
    setInboxView("projects");
    expect(rows()).toHaveLength(1);
    expect(document.querySelector("[data-done], [data-dismiss], [data-mute], [data-project-create]")).toBeNull();
    expect(document.getElementById("inbox-list").textContent).not.toMatch(/branch|issue/i);
  });
});
