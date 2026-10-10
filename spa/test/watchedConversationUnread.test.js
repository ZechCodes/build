// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { railBubbles, projectAgentBubble } from "../src/core/agentRailModel.js";
import { overviewRows, overviewSnippet, overviewState } from "../src/core/agentOverview.js";
import { projectMenuModel } from "../src/core/toolbarModel.js";
import { projectAgentEntries } from "../src/core/inboxProjectAgent.js";
import { agentsUnreadCount, watchedUnreadCount } from "../src/core/inboxRoster.js";
import { inboxEntries, workspaceEntries, watchedWorkspaceEntries } from "../src/core/inbox.js";

const project = { id: "project-1", projectKey: "device/project-1", deviceId: "device", name: "Build", entity_id: "project-run" };
const watched = { id: "watched", watched: true, unread_count: 2 };
const unwatched = { id: "unwatched", watched: false, unread_count: 9, unread_reason: "agent_message", topic: "Read scoped counts" };
const legacy = { id: "legacy", unread_count: 1 };
const row = (changes = {}) => ({ kind: "branch", project_id: project.id, projectKey: project.projectKey,
  run_id: project.entity_id, unread: true, unread_count: 12, agents: [watched, unwatched, legacy], ...changes });

describe("watched conversation unread (#474)", () => {
  it("excludes unwatched agent counts from every roster sum, preserving legacy watching", () => {
    expect(agentsUnreadCount([watched, unwatched, legacy])).toBe(3);
    expect(watchedUnreadCount([watched, unwatched, legacy])).toBe(3);
  });

  it("shows no unread bubble or unread news on the open unwatched agent", () => {
    const [bubble] = railBubbles({ agents: [unwatched], selectedId: unwatched.id, kind: "workspace" });
    expect(bubble).toMatchObject({ unread: 0, unwatched: true });
    expect(bubble.title).not.toContain("message");
    expect(projectAgentBubble({ name: "Build", agents: [watched, unwatched, legacy] }).unread).toBe(3);
    const projectBubble = projectAgentBubble({ name: "Build", agents: [unwatched], active: true });
    expect(projectBubble.unread).toBe(0);
    expect(projectBubble.title).not.toContain("message");
  });

  it("keeps an unwatched overview quiet even when a cached count or failure remains", () => {
    expect(overviewState(unwatched).state).toBe("idle");
    expect(overviewState({ ...unwatched, unread_reason: "run_failed", working: true }).state).toBe("working");
    const [entry] = overviewRows([{ agent: unwatched, source: "own" }], [null]);
    expect(entry).toMatchObject({ unread: false, unreadCount: 0, stateWord: "Idle" });
    const [staleExecution] = overviewRows([{ agent: unwatched, state: { ...unwatched, watched: true }, source: "own" }], [null]);
    expect(staleExecution).toMatchObject({ unread: false, unreadCount: 0, stateWord: "Idle" });
    const thread = { items: [
      { type: "message", data: { sequence: 1, role: "agent", body: "Old reply" } },
      { type: "message", data: { sequence: 2, role: "user", body: "Latest words" } },
    ] };
    expect(overviewSnippet(unwatched, thread)).toBe("Latest words");
  });

  it.each(["run_failed", "blocked"])("keeps a rewatched roster quiet over an alias's stale %s news", (reason) => {
    const roster = { id: "task-agent", watched: true, unread_count: 0, read_through_sequence: 0 };
    const execution = { id: "implementation-agent", watched: true, unread_count: 400,
      unread_reason: reason, read_through_sequence: 0 };
    const [entry] = overviewRows([{ agent: roster, state: { ...roster, ...execution }, source: "current" }], [null]);
    expect(entry).toMatchObject({ id: "task-agent", watching: true, unread: false, unreadCount: 0,
      state: "idle", stateWord: "Idle", stateDetail: "" });
  });

  it.each([false, true])("keeps roster unread news and execution metadata when the alias is working=%s", (working) => {
    const roster = { id: "task-agent", watched: true, unread_count: 2, unread_reason: "agent_message",
      read_through_sequence: 0, active_model: "claude-opus-5" };
    const execution = { id: "implementation-agent", watched: true, unread_count: 400, unread_reason: "run_failed",
      read_through_sequence: 0, working, provider: "codex", active_model: "gpt-6-astra", effort: "xhigh" };
    const [entry] = overviewRows([{ agent: roster, state: { ...roster, ...execution }, source: "current" }], [null]);
    expect(entry).toMatchObject({ id: "task-agent", watching: true, unread: true, unreadCount: 2,
      working, model: "6 Astra", modelName: "gpt-6-astra", effort: "xhigh",
      state: working ? "working" : "waiting", stateWord: working ? "Working" : "Unread" });
  });

  it("uses the roster's watched total in project menus and project heads", () => {
    expect(projectMenuModel({ projects: [project], items: [row()] })[0].unreadCount).toBe(3);
    const [entry] = projectAgentEntries([project], [row()], [], () => 4);
    expect(entry).toMatchObject({ ownUnreadCount: 3, unreadCount: 7 });
  });

  it("suppresses old bridge counts and unread flags when they explicitly say unwatched", () => {
    const summary = row({ agents: undefined, watched: false });
    expect(projectMenuModel({ projects: [project], items: [summary] })[0].unreadCount).toBe(0);
    expect(projectMenuModel({ projects: [project], items: [row({ agents: undefined, watched: false, unread_count: 0 })] })[0].unreadCount).toBe(0);
    expect(projectAgentEntries([project], [summary])[0].ownUnreadCount).toBe(0);
    expect(projectMenuModel({ projects: [project], items: [row({ agents: undefined, unread_count: 0 })] })[0].unreadCount).toBe(1);
  });

  it("zeroes legacy summary counts in workspace and conversation rows", () => {
    const summary = row({ agents: undefined, watched: false });
    const workspace = { id: "workspace-1", workspaceKey: "device/workspace-1", project_id: project.id,
      projectKey: project.projectKey, entity_id: project.entity_id };
    expect(workspaceEntries([workspace], [project], [summary])[0].unreadCount).toBe(0);
    expect(inboxEntries({ items: [summary] }).entries[0].unreadCount).toBe(0);
    const legacySummary = row({ agents: undefined });
    expect(workspaceEntries([workspace], [project], [legacySummary])[0].unreadCount).toBe(12);
    expect(inboxEntries({ items: [legacySummary] }).entries[0].unreadCount).toBe(12);
  });

  it("does not retain a workspace unread status from an unwatched cached summary", () => {
    const workspace = { id: "workspace-1", workspaceKey: "device/workspace-1", project_id: project.id,
      projectKey: project.projectKey, entity_id: project.entity_id };
    const [entry] = watchedWorkspaceEntries([workspace], [project], [row({ agents: [{ ...watched, unread_count: 0 }, unwatched] })]);
    expect(entry).toMatchObject({ unreadCount: 0, state: "inactive" });
  });
});
