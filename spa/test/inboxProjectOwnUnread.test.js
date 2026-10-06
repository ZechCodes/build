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
});
