// @vitest-environment jsdom
// #380: every row has one right-edge status dot, derived from its cached
// running state and unread activity, instead of a left icon and a count.
import { describe, expect, it } from "vitest";
import { inboxEntries, inboxRowHtml } from "../src/core/inbox.js";

const entry = (over = {}) => ({
  kind: "workspace", key: "workspace:dev/ws", name: "Checkout", project: "Build",
  state: "inactive", unreadCount: 0, facts: "↑0 ↓0 +0 −0", route: { name: "workspace" },
  ...over,
});
const rowOf = (row, ui) => new DOMParser().parseFromString(inboxRowHtml(row, ui), "text/html").querySelector(".inbox-entry");
const statusOf = (row) => row.querySelector(".inbox-actions > .inbox-status-dot");

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
