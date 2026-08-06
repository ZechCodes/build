// @vitest-environment jsdom
// The cluster as a surface wires it: mountTabShell's right cluster selecting
// tabs, and mountProjectClusterTab painting their bodies. Every project surface
// (main, external worktree, run, issue) spreads exactly these options, so this
// pins the contract they share.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { mountTabShell } from "../src/core/tabshell.js";
import { mountProjectClusterTab, projectClusterShellOptions } from "../src/core/projectCluster.js";

const SURFACE_TABS = [
  { id: "conversation", label: "Conversation" },
  { id: "changes", label: "Changes" },
];

const click = (element) => element.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
const menu = () => document.querySelector(".tabmenu");
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

// A miniature project surface: a tab row plus a body that mounts whatever the
// cluster owns and falls back to its own tabs otherwise.
function mountSurface(callRpc) {
  const row = document.createElement("div");
  const body = document.createElement("div");
  document.body.append(row, body);
  let pane = null;
  let active = "conversation";
  const selectTab = (tabId) => {
    active = tabId;
    shell.setActive(tabId);
    if (pane) pane.dispose();
    body.innerHTML = "";
    pane = mountProjectClusterTab(body, tabId, { projectId: "proj-1", callRpc, navigate: () => {} });
    if (!pane) body.innerHTML = `<div class="own">${tabId}</div>`;
  };
  const shell = mountTabShell(row, {
    tabs: SURFACE_TABS,
    active,
    onSelect: selectTab,
    ...projectClusterShellOptions({ projectId: "proj-1", selectTab }),
  });
  return { row, body, activeTab: () => active, dots: () => row.querySelector(".tmenu") };
}

beforeEach(() => {
  document.body.innerHTML = "";
});

describe("the right cluster on a project surface", () => {
  it("selects the Inbox icon tab and paints the project inbox in the body", async () => {
    const callRpc = vi.fn().mockResolvedValue({ runs: [], plans: [], external_worktrees: [] });
    const surface = mountSurface(callRpc);
    click(surface.row.querySelector('.ticon[data-tab="inbox"]'));
    await flush();
    expect(surface.activeTab()).toBe("inbox");
    expect(surface.row.querySelector('[data-tab="inbox"]').classList.contains("active")).toBe(true);
    expect(callRpc).toHaveBeenCalledWith("board.list");
    expect(surface.body.querySelector(".own")).toBeNull();
  });

  it("reaches Archive through the ⋯ menu, as a tab state like any other", async () => {
    const callRpc = vi.fn().mockResolvedValue({ plans: [], worktrees: [] });
    const surface = mountSurface(callRpc);
    click(surface.dots());
    click(menu().querySelector('[data-action="archive"]'));
    await flush();
    expect(surface.activeTab()).toBe("archive");
    expect(callRpc).toHaveBeenCalledWith("archive.list", { project_id: "proj-1" });
    expect(menu()).toBeNull();
  });

  it("leaves the surface's own tabs to the surface", async () => {
    const callRpc = vi.fn().mockResolvedValue({});
    const surface = mountSurface(callRpc);
    click(surface.row.querySelector('[data-tab="changes"]'));
    await flush();
    expect(surface.body.querySelector(".own").textContent).toBe("changes");
    expect(callRpc).not.toHaveBeenCalled();
  });
});
