import { describe, expect, it } from "vitest";
import { activeEntryKey, entryRoute, routedEntityId, TRACKER_TASK } from "../src/core/inbox.js";
import { watchedTaskEntries } from "../src/core/watchedTaskRows.js";

const project = { id: "p1", deviceId: "d1", projectKey: "d1/p1", name: "Build" };
const identity = { agent_id: "a1", available: true, workspace_id: "w1" };
const assignee = { kind: "agent", agent_id: "a1" };
const fallback = { name: "trackerTask", deviceId: "d1", projectId: "p1", taskId: "t1" };
const destination = { name: "workspace", deviceId: "d1", projectId: "p1", workspaceId: "w1",
  tab: "tasks", taskId: "t1", agent: "a1" };
const item = (over = {}) => ({ kind: TRACKER_TASK, task_id: "t1", deviceId: "d1", project_id: "p1",
  assignee, identities: { a1: identity }, workspaces: [{ id: "w1", project_id: "p1" }], ...over });
const task = (over = {}) => ({ id: "t1", watched: true, state: "open", status: "in_progress",
  assignee, identities: { a1: identity }, updated_at: "2026-10-12T12:00:00Z", ...over });
const watchedRoute = (one, detail = null, workspaces = [{ id: "w1", project_id: "p1" }]) =>
  watchedTaskEntries([{ project, tasks: [one], details: new Map([[one.id, detail]]), workspaces }])[0].route;

describe("inbox task destination", () => {
  it("opens the assigned workspace agent's task and conversation", () => {
    expect(entryRoute(item())).toEqual(destination);
  });

  it.each([
    ["unassigned", { assignee: null }],
    ["assigned to the user", { assignee: { kind: "user" } }],
    ["assigned to the project agent", { assignee: { kind: "project_agent" } }],
    ["assigned to an unavailable agent", { identities: { a1: { ...identity, available: false } } }],
    ["assigned to a project agent by id", { identities: { a1: { ...identity, workspace_id: null } } }],
    ["missing the cached identity", { identities: {} }],
    ["missing the cached task record", { assignee: undefined, identities: undefined }],
    ["assigned to a deleted workspace", { workspaces: [] }],
  ])("opens the project task page when %s", (_label, over) => {
    expect(entryRoute(item(over))).toEqual(fallback);
  });

  it("routes watched rows from the cached list before any detail has landed", () => {
    expect(watchedRoute(task())).toEqual(destination);
  });

  it("uses a current cached detail when the list lacks the assignee identity", () => {
    expect(watchedRoute(task({ identities: {} }), { task: task() })).toEqual(destination);
  });

  it("keeps a newer list assignment ahead of stale detail", () => {
    expect(watchedRoute(task({ assignee: null, updated_at: "2026-10-12T13:00:00Z" }), { task: task() })).toEqual(fallback);
  });

  it("does not resurrect an unavailable list identity from equal-stamp detail", () => {
    expect(watchedRoute(task({ identities: { a1: { ...identity, available: false } } }), { task: task() })).toEqual(fallback);
  });

  it("does not use a stale detail identity missing from the newer list", () => {
    expect(watchedRoute(task({ identities: {}, updated_at: "2026-10-12T13:00:00Z" }), { task: task() })).toEqual(fallback);
  });

  it("falls back after the cached workspace disappears", () => {
    expect(watchedRoute(task(), { task: task() }, [])).toEqual(fallback);
  });
});

describe("inbox standing on a workspace task", () => {
  const workspace = { kind: "workspace", key: "workspace:d1/w1", deviceId: "d1", workspaceId: "w1" };
  const entry = { kind: TRACKER_TASK, key: "tracker_task:t1", deviceId: "d1", taskId: "t1" };

  it("marks the task entry current even when its workspace row comes first", () => {
    expect(activeEntryKey(destination, [workspace, entry])).toBe(entry.key);
    expect(activeEntryKey({ ...destination, deviceId: "other" }, [workspace, entry])).toBeNull();
    expect(activeEntryKey({ ...destination, taskId: undefined }, [workspace, entry])).toBe(workspace.key);
  });

  it("marks the workspace current when the open task has no inbox row", () => {
    expect(activeEntryKey(destination, [workspace])).toBe(workspace.key);
    expect(activeEntryKey(destination, [workspace, { ...entry, taskId: "unrelated" }])).toBe(workspace.key);
  });

  it("keeps the workspace conversation as the routed entity for cache subscriptions", () => {
    const view = { projects: [project], items: [], workspaces: [{ id: "w1", deviceId: "d1", project_id: "p1",
      projectKey: project.projectKey, workspaceKey: "d1/w1", entity_id: "run1", status: "ready" }] };
    expect(routedEntityId(destination, view)).toBe("run1");
  });
});
