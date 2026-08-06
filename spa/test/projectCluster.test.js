// @vitest-environment jsdom
// The tab-bar right cluster every project surface carries: the Inbox icon tab
// and the ⋯ menu (Archive, Project settings). One definition, one body-mounting
// entry point — the surfaces only route.

import { describe, it, expect, vi } from "vitest";
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
  it("puts Inbox on the row as an icon tab with an accessible name", () => {
    expect(PROJECT_CLUSTER_TABS).toEqual([{ id: "inbox", glyph: "▤", label: "Inbox" }]);
  });

  it("puts Archive and Project settings behind the ⋯", () => {
    expect(PROJECT_CLUSTER_MENU.map((item) => item.id)).toEqual(["archive", "settings"]);
    expect(PROJECT_CLUSTER_MENU.map((item) => item.label)).toEqual(["Archive", "Project settings"]);
  });

  it("claims only its own tab ids", () => {
    expect(isProjectClusterTab("inbox")).toBe(true);
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

  it("returns null for a tab it does not own, so the surface mounts its own", () => {
    const pane = mountProjectClusterTab(hostElement(), "changes", {
      projectId: "proj-1",
      callRpc: vi.fn(),
      navigate: () => {},
    });
    expect(pane).toBeNull();
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
