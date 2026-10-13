import { describe, expect, it } from "vitest";
import { projectAgentEntries } from "../src/core/inboxProjectAgent.js";

const project = { id: "project-1", projectKey: "device/project-1", deviceId: "device", name: "Build", entity_id: "project-run" };

describe("the project agent's own unread activity", () => {
  it("keeps conversation unread separate from watched task news for the expanded head", () => {
    const item = { kind: "branch", project_id: project.id, projectKey: project.projectKey,
      run_id: project.entity_id, agents: [{ id: "agent-1", watched: true, unread_count: 2, working: true }] };
    const [entry] = projectAgentEntries([project], [item], [], () => 5);
    expect(entry.ownUnreadCount).toBe(2);
    expect(entry.unreadCount).toBe(7);
    expect(entry.working).toBe(true);
  });

  it("uses the cached conversation summary when no roster has landed", () => {
    const item = { kind: "branch", project_id: project.id, projectKey: project.projectKey,
      run_id: project.entity_id, unread_count: 3, working: false };
    const [entry] = projectAgentEntries([project], [item], [], () => 4);
    expect(entry.ownUnreadCount).toBe(3);
    expect(entry.unreadCount).toBe(7);
  });

  it("does not attribute task-only unread to the project conversation", () => {
    const [entry] = projectAgentEntries([project], [], [], () => 4);
    expect(entry.ownUnreadCount).toBe(0);
    expect(entry.unreadCount).toBe(4);
  });

  it("keeps the folded watched summary separate from an unwatched agent's own work", () => {
    const item = { kind: "branch", project_id: project.id, projectKey: project.projectKey,
      run_id: project.entity_id, agents: [{ id: "watched", watched: true, working: false },
        { id: "unwatched", watched: false, working: true }] };
    const [entry] = projectAgentEntries([project], [item]);
    expect(entry.working).toBe(true);
    expect(entry.watchedWorking).toBe(false);
  });
});


describe("cached project worker unread (#536)", () => {
  const pin = { id: "pin", conversation_id: "pin-history", watched: true, unread_count: 2 };
  const worker = { id: "worker", conversation_id: "worker-history", watched: true, unread_count: 3, working: true };
  const withRoster = (agents) => ({ ...project, projectRoster: {
    entity_id: project.entity_id, pinned_agent_id: pin.id, agents,
  } });

  it("counts each watched canonical conversation once, separate from task news", () => {
    const unseen = { id: "unwatched", watched: false, unread_count: 20, working: true };
    const alias = { ...worker, id: "alias" };
    const stale = { projectKey: project.projectKey, run_id: project.entity_id, agents: [{ ...pin, unread_count: 50 }] };
    const [entry] = projectAgentEntries([withRoster([pin, worker, alias, unseen])], [stale], [stale], () => 4);
    expect(entry).toMatchObject({ ownUnreadCount: 5, unreadCount: 9, working: true, watchedWorking: true });
  });

  it("does not revive stale summary news after the cached complete roster becomes empty", () => {
    const stale = { projectKey: project.projectKey, run_id: project.entity_id, agents: [worker], unread_count: 30, working: true };
    const [entry] = projectAgentEntries([withRoster([])], [stale]);
    expect(entry).toMatchObject({ ownUnreadCount: 0, unreadCount: 0, working: false, watchedWorking: false });
  });
});
