// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "fake-indexeddb/auto";
import { followNavigationTaskUnread } from "../src/core/navigationTaskUnread.js";
import { deleteCached, evictEntity, wipeCache, writeCached } from "../src/core/localCache.js";
import { taskAddress, taskRecord, tasksAddress, tasksRecord } from "../src/core/trackerCache.js";
import { comment, task } from "./trackerWireFixture.js";

const forbidden = vi.hoisted(() => vi.fn(() => { throw new Error("Navigation unread must use cache alone"); }));
vi.mock("../src/core/deviceContexts.js", () => ({ contextFor: forbidden, whenGreeted: forbidden }));

const project = { id: "p1", deviceId: "device", projectKey: "device/p1" };
const other = { id: "p1", deviceId: "other-device", projectKey: "other-device/p1" };
const watched = (id, unread_count, extra = {}) => task({ id, watched: true, unread_count, ...extra });
const putTasks = (owner, tasks) => writeCached(tasksAddress(owner.deviceId, owner.id), tasksRecord(tasks));
const putDetail = (owner, held, timeline) => writeCached(taskAddress(owner.deviceId, owner.id, held.id), taskRecord(held, timeline));
const agentSays = (id) => comment({ id, author: { kind: "agent", agent_id: "workspace-agent" } });
let follower;

beforeEach(async () => { await wipeCache(); forbidden.mockClear(); });
afterEach(() => { follower?.dispose(); follower = null; expect(forbidden).not.toHaveBeenCalled(); });

const total = (key = project.projectKey) => follower.taskUnread().unheldBy([])(key);

describe("navigation task unread from cache", () => {
  it("assigns watched tasks to their project or workspace agents across cached devices", async () => {
    await putTasks(project, [
      watched("project-task", 2, { assignee: { kind: "project_agent" } }),
      watched("workspace-task", 3, { assignee: { kind: "agent", agent_id: "workspace-agent" } }),
      watched("unwatched-task", 9, { watched: false }),
      watched("done-task", 8, { status: "done" }),
      watched("closed-task", 7, { state: "closed" }),
    ]);
    await putTasks(other, [watched("workspace-task", 4)]);
    follower = followNavigationTaskUnread();
    follower.follow([project, other]);
    await vi.waitFor(() => expect(total()).toBe(5));
    await vi.waitFor(() => expect(total(other.projectKey)).toBe(4));
    const tally = follower.taskUnread();
    const workspace = { projectKey: project.projectKey, agentIds: ["workspace-agent"] };
    expect(tally.heldBy(project.projectKey, workspace.agentIds)).toBe(3);
    expect(tally.unheldBy([workspace])(project.projectKey)).toBe(2);
    expect(tally.heldBy(other.projectKey, workspace.agentIds)).toBe(0);
  });

  it("reads a cached timeline and follows later read marks without a list tick", async () => {
    const held = watched("detail-task", 9, { read_through: "tc-01" });
    await putTasks(project, [held]);
    await putDetail(project, held, [agentSays("tc-01"), agentSays("tc-02"), agentSays("tc-03")]);
    const onChange = vi.fn();
    follower = followNavigationTaskUnread({ onChange });
    follower.follow([project]);
    await vi.waitFor(() => expect(total()).toBe(2));
    onChange.mockClear();
    await putDetail(project, { ...held, read_through: "tc-03" }, [agentSays("tc-01"), agentSays("tc-02"), agentSays("tc-03")]);
    await vi.waitFor(() => expect(total()).toBe(0));
    expect(onChange).toHaveBeenCalled();
  });

  it("uses the list while details lag, then adopts the cache's newer timeline", async () => {
    const old = watched("detail-task", 1, { updated_at: "2026-08-21T10:00:00Z" });
    const next = { ...old, updated_at: "2026-08-21T11:00:00Z", unread_count: 5 };
    await putTasks(project, [next]);
    await putDetail(project, old, [agentSays("tc-01")]);
    follower = followNavigationTaskUnread();
    follower.follow([project]);
    await vi.waitFor(() => expect(total()).toBe(5));
    await putDetail(project, next, [agentSays("tc-01"), agentSays("tc-02")]);
    await vi.waitFor(() => expect(total()).toBe(2));
    await deleteCached([taskAddress(project.deviceId, project.id, next.id)]);
    await vi.waitFor(() => expect(total()).toBe(5));
  });

  it("reacts to assignments, unwatches and removals even without new update stamps", async () => {
    const held = watched("task", 3, { assignee: { kind: "agent", agent_id: "workspace-agent" } });
    await putTasks(project, [held]);
    follower = followNavigationTaskUnread();
    follower.follow([project]);
    await vi.waitFor(() => expect(follower.taskUnread().heldBy(project.projectKey, ["workspace-agent"])).toBe(3));
    await putTasks(project, [{ ...held, assignee: { kind: "project_agent" } }]);
    await vi.waitFor(() => expect(follower.taskUnread().heldBy(project.projectKey, ["workspace-agent"])).toBe(0));
    expect(total()).toBe(3);
    await putTasks(project, [{ ...held, watched: false }]);
    await vi.waitFor(() => expect(total()).toBe(0));
    await putTasks(project, [held]);
    await vi.waitFor(() => expect(total()).toBe(3));
    await putTasks(project, []);
    await vi.waitFor(() => expect(total()).toBe(0));
  });

  it("drops removed projects and stops cache notifications after disposal", async () => {
    await putTasks(project, [watched("task", 2)]);
    await putTasks(other, [watched("task", 4)]);
    const onChange = vi.fn();
    follower = followNavigationTaskUnread({ onChange });
    follower.follow([project, other]);
    await vi.waitFor(() => expect(total()).toBe(2));
    await vi.waitFor(() => expect(total(other.projectKey)).toBe(4));
    follower.follow([other]);
    expect(total()).toBe(0);
    expect(total(other.projectKey)).toBe(4);
    await evictEntity(other.deviceId, other.id);
    await vi.waitFor(() => expect(total(other.projectKey)).toBe(0));
    follower.dispose();
    onChange.mockClear();
    await putTasks(other, [watched("task", 7)]);
    expect(total(other.projectKey)).toBe(0);
    expect(onChange).not.toHaveBeenCalled();
  });
});
