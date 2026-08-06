// @vitest-environment jsdom
// The tab-bar right cluster every project surface carries: the Inbox and Issues
// icon tabs and the ⋯ menu (Archive, Project settings). One definition, one
// body-mounting entry point — the surfaces only route.

import { describe, it, expect, vi } from "vitest";
import { ICON_CIRCLE_DOT, ICON_INBOX } from "../src/core/icons.js";
import {
  PROJECT_CLUSTER_TABS,
  PROJECT_CLUSTER_MENU,
  isProjectClusterTab,
  mountProjectClusterTab,
  projectClusterShellOptions,
} from "../src/core/projectCluster.js";

const hostElement = () => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  return host;
};

describe("the project cluster's entries", () => {
  // Every project-scoped pane is an icon tab here; the surfaces' own tabs
  // (conversation/changes/files/agent/terminals) keep the row's left side.
  it("puts Inbox and Issues on the row as icon tabs with accessible names", () => {
    expect(PROJECT_CLUSTER_TABS).toEqual([
      { id: "inbox", icon: ICON_INBOX, label: "Inbox" },
      { id: "issues", icon: ICON_CIRCLE_DOT, label: "Issues" },
    ]);
  });

  // Recognisable at a glance is the whole point of an icon-only cell: a tray for
  // the inbox, and the circle-dot every issue tracker uses for issues.
  it("draws them from the icon set, not from unicode", () => {
    expect(ICON_INBOX).toContain("lucide-inbox");
    expect(ICON_CIRCLE_DOT).toContain("lucide-circle-dot");
    for (const tab of PROJECT_CLUSTER_TABS) {
      expect(tab.icon).toContain("<svg");
      expect(tab.icon).toContain('stroke="currentColor"');
    }
  });

  it("puts Archive and Project settings behind the ⋯", () => {
    expect(PROJECT_CLUSTER_MENU.map((item) => item.id)).toEqual(["archive", "settings"]);
    expect(PROJECT_CLUSTER_MENU.map((item) => item.label)).toEqual(["Archive", "Project settings"]);
  });

  it("claims only its own tab ids", () => {
    expect(isProjectClusterTab("inbox")).toBe(true);
    expect(isProjectClusterTab("issues")).toBe(true);
    expect(isProjectClusterTab("archive")).toBe(true);
    expect(["conversation", "changes", "files", "agent", "term-1", undefined].map(isProjectClusterTab)).toEqual([
      false,
      false,
      false,
      false,
      false,
      false,
    ]);
  });
});

describe("mountProjectClusterTab", () => {
  it("mounts the project inbox for the inbox tab", async () => {
    const callRpc = vi.fn().mockResolvedValue({ runs: [], plans: [], external_worktrees: [] });
    const pane = mountProjectClusterTab(hostElement(), "inbox", { projectId: "proj-1", callRpc, navigate: () => {} });
    expect(pane).not.toBeNull();
    await Promise.resolve();
    expect(callRpc).toHaveBeenCalledWith("board.list");
    pane.dispose();
  });

  it("mounts the archive for the archive tab, scoped to the project", async () => {
    const callRpc = vi.fn().mockResolvedValue({ plans: [], worktrees: [] });
    const pane = mountProjectClusterTab(hostElement(), "archive", { projectId: "proj-1", callRpc, navigate: () => {} });
    expect(pane).not.toBeNull();
    await Promise.resolve();
    expect(callRpc).toHaveBeenCalledWith("archive.list", { project_id: "proj-1" });
    pane.dispose();
  });

  it("mounts the project's issues for the issues tab, with the sheet behind its verb", async () => {
    const callRpc = vi.fn().mockResolvedValue({ plans: [] });
    const filed = [];
    const host = hostElement();
    const pane = mountProjectClusterTab(host, "issues", {
      projectId: "proj-1",
      callRpc,
      navigate: () => {},
      openNewIssue: (options) => filed.push(options),
    });
    expect(pane).not.toBeNull();
    await Promise.resolve();
    expect(callRpc).toHaveBeenCalledWith("board.list");
    host.querySelector("[data-newissue]").click();
    expect(filed).toEqual([{ projectId: "proj-1" }]);
    pane.dispose();
  });

  it("routes an issue row to that issue's surface", async () => {
    const plan = { plan_id: "pl-1", project_id: "proj-1", goal: "ship it", state: "plan_review", stages: [] };
    const callRpc = vi.fn().mockResolvedValue({ plans: [plan] });
    const host = hostElement();
    const routes = [];
    const pane = mountProjectClusterTab(host, "issues", {
      projectId: "proj-1",
      callRpc,
      navigate: (route) => routes.push(route),
    });
    await Promise.resolve();
    host.querySelector(".issue-row").click();
    expect(routes).toEqual([{ name: "plan", projectId: "proj-1", id: "pl-1", tab: "conversation" }]);
    pane.dispose();
  });

  // The row shows an icon and nothing else, so the pane is where the user reads
  // which one they are standing in. One heading, from here, on every surface.
  it("names each pane in a heading of its own", async () => {
    const callRpc = vi.fn().mockResolvedValue({ runs: [], plans: [], external_worktrees: [] });
    for (const [tabId, title] of [
      ["inbox", "Inbox"],
      ["issues", "Issues"],
      ["archive", "Archive"],
    ]) {
      const host = hostElement();
      const pane = mountProjectClusterTab(host, tabId, { projectId: "proj-1", callRpc, navigate: () => {} });
      await Promise.resolve();
      const headings = host.querySelectorAll(".board-head h1");
      expect(headings).toHaveLength(1);
      expect(headings[0].textContent).toBe(title);
      pane.dispose();
    }
  });

  it("mounts the pane under its heading, not over it", async () => {
    const callRpc = vi.fn().mockResolvedValue({ plans: [] });
    const host = hostElement();
    const pane = mountProjectClusterTab(host, "issues", { projectId: "proj-1", callRpc, navigate: () => {} });
    await Promise.resolve();
    expect(host.querySelector(".board-head h1").textContent).toBe("Issues");
    expect(host.querySelector(".issues")).toBeTruthy();
    pane.dispose();
  });

  it("mounts nothing until the surface has learned its project", () => {
    for (const tabId of ["inbox", "issues", "archive"]) {
      expect(mountProjectClusterTab(hostElement(), tabId, { projectId: null, callRpc: vi.fn(), navigate: () => {} })).toBeNull();
    }
  });

  it("returns null for a tab it does not own, so the surface mounts its own", () => {
    const host = hostElement();
    const pane = mountProjectClusterTab(host, "changes", {
      projectId: "proj-1",
      callRpc: vi.fn(),
      navigate: () => {},
    });
    expect(pane).toBeNull();
    expect(host.innerHTML).toBe(""); // not even a heading: the body is the surface's
  });
});

describe("projectClusterShellOptions", () => {
  it("hands the shell the same cluster on every surface", () => {
    const options = projectClusterShellOptions({ projectId: "proj-1", selectTab: () => {} });
    expect(options.rightTabs).toEqual(PROJECT_CLUSTER_TABS);
    expect(options.menu).toEqual(PROJECT_CLUSTER_MENU);
  });

  it("routes Archive to the surface's own tab selection — the menu is only its entry point", () => {
    const selected = [];
    const options = projectClusterShellOptions({ projectId: "proj-1", selectTab: (id) => selected.push(id) });
    options.onMenuPick("archive");
    expect(selected).toEqual(["archive"]);
  });

  it("opens the project settings sheet for its own item, selecting no tab", () => {
    const selected = [];
    const opened = [];
    const options = projectClusterShellOptions({
      projectId: "proj-1",
      selectTab: (id) => selected.push(id),
      openSettings: (projectId) => opened.push(projectId),
    });
    options.onMenuPick("settings");
    expect(opened).toEqual(["proj-1"]);
    expect(selected).toEqual([]);
  });

  it("offers no cluster until the surface knows which project it belongs to", () => {
    expect(projectClusterShellOptions({ projectId: null, selectTab: () => {} })).toEqual({});
  });
});
