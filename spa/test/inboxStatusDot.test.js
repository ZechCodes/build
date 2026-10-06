// @vitest-environment jsdom
// #380: every row has one right-edge status dot, derived from its cached
// running state and unread activity, instead of a left icon and a count.
import { describe, expect, it } from "vitest";
import { inboxEntries, inboxRowHtml, watchedWorkspaceEntries, workspaceEntries } from "../src/core/inbox.js";
import { projectAgentEntries } from "../src/core/inboxProjectAgent.js";
import { projectHeadHtml, workspaceProjectBlocks } from "../src/core/inboxProjects.js";
import { runningAgentCount } from "../src/core/inboxRoster.js";

const entry = (over = {}) => ({
  kind: "workspace", key: "workspace:dev/ws", name: "Checkout", project: "Build",
  state: "inactive", unreadCount: 0, facts: "↑0 ↓0 +0 −0", route: { name: "workspace" },
  ...over,
});
const rowOf = (row, ui) => new DOMParser().parseFromString(inboxRowHtml(row, ui), "text/html").querySelector(".inbox-entry");
const statusOf = (row) => row.querySelector(".inbox-actions > .inbox-status-dot");

describe("inbox running activity while an agent waits on another agent (#386)", () => {
  const project = { id: "proj-386", projectKey: "dev/proj-386", deviceId: "dev", name: "Build", entity_id: "project-run" };
  const workspace = { id: "ws-386", workspaceKey: "dev/ws-386", project_id: project.id, projectKey: project.projectKey,
    deviceId: project.deviceId, name: "Checkout", status: "ready", entity_id: "workspace-run" };
  const waiting = (id, over = {}) => ({ id, working: false, agents_running: 1, watched: true, unread_count: 0, ...over });
  const conversation = (runId, over = {}) => ({ kind: "branch", run_id: runId, project_id: project.id,
    projectKey: project.projectKey, deviceId: project.deviceId, working: false, ...over });
  const headDot = (entry, folded) => {
    const { blocks } = workspaceProjectBlocks([entry], [project]);
    const html = projectHeadHtml(blocks[0], { folded: new Set(folded ? [project.projectKey] : []) });
    return new DOMParser().parseFromString(html, "text/html").querySelector(".inbox-status-dot");
  };

  it.each([0, 2])("pulses the project conversation and expanded/folded heads with %i unread", (unread_count) => {
    const [entry] = projectAgentEntries([project], [conversation(project.entity_id, {
      agents: [waiting("project-agent", { unread_count })],
    })]);
    expect(entry).toMatchObject({ working: true, watchedWorking: true, ownUnreadCount: unread_count });
    for (const dot of [statusOf(rowOf(entry)), headDot(entry, false), headDot(entry, true)]) {
      expect(dot?.classList.contains("inbox-status-running")).toBe(true);
      expect(dot?.classList.contains("inbox-status-unread")).toBe(unread_count > 0);
    }
  });

  it("keeps an unwatched project agent's descendants out of the folded watched summary", () => {
    const [entry] = projectAgentEntries([project], [conversation(project.entity_id, {
      agents: [waiting("watched", { agents_running: 0 }), waiting("unwatched", { watched: false })],
    })]);
    expect(entry).toMatchObject({ working: true, watchedWorking: false });
    expect(headDot(entry, false)?.classList.contains("inbox-status-running")).toBe(true);
    expect(headDot(entry, true)).toBeNull();
  });

  it("counts each running parent once and filters watching only for the folded summary", () => {
    const agents = [waiting("watched", { agents_running: 4, unread_count: 2 }), waiting("unwatched", { watched: false }),
      waiting("idle", { agents_running: 0 })];
    expect(runningAgentCount(agents)).toBe(2);
    const [entry] = watchedWorkspaceEntries([workspace], [project], [conversation(workspace.entity_id, { agents })]);
    expect(entry).toMatchObject({ working: true, watchedWorking: true, runningCount: 2, unreadCount: 2 });
    expect(entry.facts).toContain("2 running");
    expect(statusOf(rowOf(entry))?.classList.contains("inbox-status-running")).toBe(true);

    const [unwatchedOnly] = watchedWorkspaceEntries([workspace], [project], [conversation(workspace.entity_id, {
      agents: [waiting("watched", { agents_running: 0 }), waiting("unwatched", { watched: false })],
    })]);
    expect(unwatchedOnly).toMatchObject({ working: true, watchedWorking: false, runningCount: 1 });
    expect(statusOf(rowOf(unwatchedOnly))?.classList.contains("inbox-status-running")).toBe(true);
    expect(headDot(unwatchedOnly, true)).toBeNull();
  });

  it("uses the feed summary while the workspace roster has not landed", () => {
    const [entry] = workspaceEntries([workspace], [project], [conversation(workspace.entity_id, { agents_running: 1 })]);
    expect(entry).toMatchObject({ working: true, state: "working" });
    expect(statusOf(rowOf(entry))?.classList.contains("inbox-status-running")).toBe(true);
  });

  it("uses the feed summary while the project roster has not landed", () => {
    const [entry] = projectAgentEntries([project], [conversation(project.entity_id, { agents_running: 1 })]);
    expect(entry).toMatchObject({ working: true, watchedWorking: true, state: "working" });
    expect(headDot(entry, false)?.classList.contains("inbox-status-running")).toBe(true);
  });
});

describe("the inbox's shared status dot", () => {
  it.each(["workspace", "branch", "task", "tracker_task", "project_agent"])("puts %s unread activity after the actions with no left icon or counter", (kind) => {
    const row = rowOf(entry({ kind, state: "unread", unreadCount: 12 }));
    const dot = statusOf(row);
    expect(dot).not.toBeNull();
    expect(dot.classList.contains("inbox-status-unread")).toBe(true);
    expect(dot.classList.contains("inbox-status-running")).toBe(false);
    expect(dot.getAttribute("aria-label")).toMatch(/unread/i);
    expect(row.querySelector(".inbox-actions").lastElementChild).toBe(dot);
    expect(row.querySelectorAll(".inbox-status-dot")).toHaveLength(1);
    expect(row.querySelector(".sdot, .inbox-unread")).toBeNull();
  });

  it.each([
    ["running and read", { state: "working" }, true, false],
    ["running with unread activity", { state: "working", unreadCount: 2 }, true, true],
    ["read and idle", {}, false, false],
    ["waiting for the user after reading", { state: "unread", unreadCount: 0 }, false, false],
    ["running with an unread state", { state: "unread", working: true, unreadCount: 2 }, true, true],
  ])("paints %s from the row's cached execution and unread activity", (_name, over, running, unread) => {
    const row = rowOf(entry(over));
    const dot = statusOf(row);
    expect(row.querySelector(".inbox-actions")).not.toBeNull();
    expect(Boolean(dot)).toBe(running || unread);
    if (!dot) return;
    expect(dot.classList.contains("inbox-status-running")).toBe(running);
    expect(dot.classList.contains("inbox-status-unread")).toBe(unread);
    if (running) expect(dot.getAttribute("aria-label")).toMatch(/running/i);
  });

  it("keeps execution independent when normalization gives unread priority", () => {
    const { entries } = inboxEntries({ items: [{ kind: "branch", run_id: "run-1", state: "building", working: true,
      unread: true, unread_count: 3, branch: "build/login", project_id: "proj-1", deviceId: "dev-1" }] });
    expect(entries[0]).toMatchObject({ state: "unread", working: true, unreadCount: 3 });
    const dot = statusOf(rowOf(entries[0]));
    expect(dot?.classList.contains("inbox-status-running")).toBe(true);
    expect(dot?.classList.contains("inbox-status-unread")).toBe(true);
  });

  it("keeps the workspace's Done before the final dot", () => {
    const row = rowOf(entry({ ready: true, canFinish: true, state: "working" }));
    expect(row.querySelector("[data-workspace-done]").textContent).toBe("Done");
    expect(row.querySelector(".inbox-actions").lastElementChild).toBe(statusOf(row));
  });

  it("keeps a branch's open menu before the final dot", () => {
    const row = rowOf(entry({ kind: "branch", canFinish: true, state: "working" }), { openMenuKey: "workspace:dev/ws" });
    expect(row.querySelector("[data-menu]")).not.toBeNull();
    expect(row.querySelector('[role="menu"]')).not.toBeNull();
    expect(row.querySelector(".inbox-actions").lastElementChild).toBe(statusOf(row));
  });

  it.each(["workspace", "project_agent"])("keeps the %s Recent shape and puts its unread dot on the right", (kind) => {
    const row = rowOf(entry({ kind, state: "unread", unreadCount: 3 }), { quiet: true });
    expect(row.classList.contains("inbox-quiet")).toBe(true);
    expect(statusOf(row)?.classList.contains("inbox-status-unread")).toBe(true);
    expect(row.querySelector(".sdot, .inbox-unread")).toBeNull();
  });
});

describe("a capture's status dot", () => {
  const capture = (over = {}) => entry({ kind: "capture", captureId: "capture-1", captureState: "routing", state: "working", working: false, ...over });

  it.each(["queued", "unrouted", "routing"])("does not imply an active agent from %s alone", (captureState) => {
    const row = rowOf(capture({ captureState }));
    expect(statusOf(row)).toBeNull();
    expect(row.querySelector(".sdot, .capture-spinner, .inbox-unread")).toBeNull();
  });

  it("pulses while an agent is routing without duplicating the routing spinner", () => {
    const row = rowOf(capture({ working: true }));
    expect(statusOf(row)?.classList.contains("inbox-status-running")).toBe(true);
    expect(row.querySelector(".sdot, .capture-spinner")).toBeNull();
  });

  it("keeps a router question readable with a solid unread dot", () => {
    const row = rowOf(capture({ captureState: "unrouted", working: true, question: "Which project?", unreadCount: 1 }));
    expect(row.textContent).toContain("Which project?");
    expect(row.textContent).toContain("Waiting for your answer");
    expect(statusOf(row)?.classList.contains("inbox-status-unread")).toBe(true);
    expect(statusOf(row)?.classList.contains("inbox-status-running")).toBe(false);
    expect(row.querySelector(".capture-spinner")).toBeNull();
  });

  it("shows unread capture activity when older cached records have no count", () => {
    const row = rowOf(capture({ captureState: "failed", state: "unread" }));
    expect(statusOf(row)?.classList.contains("inbox-status-unread")).toBe(true);
  });

  it("keeps Retry before a failed capture's unread dot", () => {
    const row = rowOf(capture({ captureState: "failed", state: "unread", unreadCount: 1 }));
    expect(row.querySelector("[data-capture-retry]").textContent).toBe("Retry");
    expect(statusOf(row)?.classList.contains("inbox-status-unread")).toBe(true);
    expect(row.querySelector(".inbox-actions").lastElementChild).toBe(statusOf(row));
  });

  it("leaves a read router question without a dot", () => {
    const row = rowOf(capture({ question: "Which project?" }));
    expect(statusOf(row)).toBeNull();
    expect(row.querySelector(".sdot, .capture-spinner")).toBeNull();
  });
});
