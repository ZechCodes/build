// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const deviceId = "worker-device";
const entityId = "project-owner";
const project = { id: "project-1", projectKey: `${deviceId}|project-1`, deviceId, entity_id: entityId };
const pin = { id: "pin", conversation_id: "pin-history", working: false };
const worker = { id: "worker", conversation_id: "worker-history", working: true };
const roster = (agents = [worker, pin]) => ({ entity_id: entityId, pinned_agent_id: pin.id, agents });
const feed = (agents = [worker, pin]) => ({ projects: [{ ...project, projectRoster: roster(agents) }], items: [] });
const line = (body) => ({ items: [{ type: "message", data: { role: "agent", body } }] });
let cache, activity, follower;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
});
afterEach(() => { activity?.dispose(); follower?.dispose(); activity = null; follower = null; });

const writeThread = (sub, body, owner = entityId) => cache.writeCached({ deviceId, entityId: owner, kind: "thread", sub }, line(body));
const writeRoster = (agents) => cache.writeCached({ deviceId, entityId, kind: "project-agents" }, roster(agents));
const writeSupport = (projectWorkers) => cache.writeCached({ deviceId, entityId: "", kind: "project-agents-support" }, { projectWorkers });

describe("project worker cached activity (#536)", () => {
  it("reads and follows only the working worker's exact canonical history without an owner feed row", async () => {
    await writeThread(pin.conversation_id, "Coordinator history");
    await writeThread(worker.id, "Wrong agent-addressed history");
    await writeThread(worker.conversation_id, "Worker history");
    await writeThread(worker.conversation_id, "Other owner", "other-owner");
    const { createTrackerAgentActivityFeed } = await import("../src/core/trackerAgentActivityFeed.js");
    activity = createTrackerAgentActivityFeed({ deviceId });
    await activity.updateFeed(feed(), project.projectKey);
    expect([...activity.read()]).toEqual([[worker.id, "Worker history"]]);
    await writeThread(worker.conversation_id, "Worker update");
    await vi.waitFor(() => expect(activity.read().get(worker.id)).toBe("Worker update"));
    await activity.updateFeed(feed([pin]), project.projectKey);
    expect(activity.read().size).toBe(0);
  });

  it("drops a worker snippet when reset changes its canonical history", async () => {
    await writeThread(worker.conversation_id, "Old worker history");
    const { createTrackerAgentActivityFeed } = await import("../src/core/trackerAgentActivityFeed.js");
    activity = createTrackerAgentActivityFeed({ deviceId });
    await activity.updateFeed(feed(), project.projectKey);
    expect(activity.read().get(worker.id)).toBe("Old worker history");
    const reset = { ...worker, conversation_id: "reset-history" };
    await writeThread(reset.conversation_id, "New worker history");
    await activity.updateFeed(feed([pin, reset]), project.projectKey);
    expect(activity.read().get(worker.id)).toBe("New worker history");
    await writeThread(worker.conversation_id, "Late old history");
    expect(activity.read().get(worker.id)).toBe("New worker history");
  });
});

describe("watched tasks follow cached worker rosters (#536)", () => {
  async function start(assignee) {
    await cache.writeCached({ deviceId, entityId: "", kind: "projects" }, [project]);
    await writeSupport(true);
    await writeRoster([worker, pin]);
    const tracker = await import("../src/core/trackerCache.js");
    await tracker.writeTasksRecord(deviceId, project.id, tracker.tasksRecord([
      { id: "watched-task", watched: true, status: "backlog", state: "open", assignee },
    ], []));
    const { followWatchedTasks } = await import("../src/core/watchedTaskFollower.js");
    const onChange = vi.fn();
    follower = followWatchedTasks({ onChange });
    follower.follow([project]);
    await vi.waitFor(() => expect(onChange).toHaveBeenCalled());
  }

  it("recomputes exact worker running state when its roster or cached support changes", async () => {
    await start({ kind: "agent", agent_id: worker.id });
    await vi.waitFor(() => expect(follower.runningProjectKeys().has(project.projectKey)).toBe(true));
    await writeRoster([{ ...worker, working: false }, pin]);
    await vi.waitFor(() => expect(follower.runningProjectKeys().size).toBe(0));
    await writeRoster([worker, pin]);
    await vi.waitFor(() => expect(follower.runningProjectKeys().size).toBe(1));
    await writeSupport(false);
    await vi.waitFor(() => expect(follower.runningProjectKeys().size).toBe(0));
  });

  it("keeps a legacy project_agent task idle when only the unpinned worker is running", async () => {
    await start({ kind: "project_agent" });
    expect(follower.runningProjectKeys().size).toBe(0);
    await writeRoster([worker, { ...pin, working: true }]);
    await vi.waitFor(() => expect(follower.runningProjectKeys().size).toBe(1));
  });
});
