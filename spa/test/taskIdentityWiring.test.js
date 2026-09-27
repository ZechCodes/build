/** @vitest-environment jsdom */
// The bridge contract, real cache, and real task renderer in one path.
import { beforeEach, describe, expect, it } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import answer from "../../fixtures/api/v1/tasks.get.json";
import { taskRecord, readTaskRecord, writeTaskRecord } from "../src/core/trackerCache.js";
import { timelineRows } from "../src/core/trackerTimeline.js";
import { taskPageHtml } from "../src/core/trackerTaskRender.js";
import { taskLinkRows } from "../src/core/trackerLinks.js";
import { taskNoticeLineHtml } from "../src/core/trackerNotice.js";
import { taskRowHtml } from "../src/core/trackerListRender.js";
import { taskCardHtml } from "../src/core/trackerBoardRender.js";

const DEVICE = "dev-1";
const PROJECT = "proj-1";
const AGENT = "agent-01K5ZQ8M4T0J7WQ2R6X3YB9C4E";
const place = { deviceId: DEVICE, projectId: PROJECT, projectKey: "dev-1|proj-1" };
const workspace = { workspace_id: "ws-3f2a91c4", id: "ws-3f2a91c4", name: "spa-flaky-tests", projectKey: place.projectKey };

const renderCached = async (feed = { workspaces: [workspace] }) => {
  const record = await readTaskRecord(DEVICE, PROJECT, answer.params.task_id);
  const host = document.createElement("div");
  host.innerHTML = taskPageHtml(record.task, {
    ...place,
    projectName: "Build",
    identities: record.task.identities,
    rows: timelineRows(record.timeline),
    links: taskLinkRows(record.task, place, feed),
    columns: [], draft: "", labelsDraft: "", busy: false, sending: false,
  });
  return host;
};

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
});

describe("bridge identity through the task cache", () => {
  it("names an unwatched comment author, draws its harness, and links its agent", async () => {
    // No feed item or watched-agent digest is involved in this path.
    await writeTaskRecord(DEVICE, PROJECT, answer.params.task_id,
      taskRecord(answer.result.task, answer.result.timeline));
    const host = await renderCached();
    const author = host.querySelector(".task-comment .task-entry-head strong a");
    expect(author.textContent).toBe("spa-flaky-tests · Fix drag");
    expect(author.href).toContain(`agent=${AGENT}`);
    expect(host.querySelector(".task-comment .rail-harness-icon")?.dataset.harnessIcon).toBe("codex_app_server");
  });

  it("keeps a finished workspace's identity and icon but leaves deleted destinations as text", async () => {
    const task = structuredClone(answer.result.task);
    task.identities[AGENT].available = false;
    await writeTaskRecord(DEVICE, PROJECT, answer.params.task_id,
      taskRecord(task, answer.result.timeline));
    const host = await renderCached({ workspaces: [] });
    expect(host.querySelector(".task-comment .task-entry-head strong").textContent).toBe("spa-flaky-tests · Fix drag");
    expect(host.querySelector(".task-comment .task-entry-head a")).toBeNull();
    expect(host.querySelector(".task-comment .rail-harness-icon")?.dataset.harnessIcon).toBe("codex_app_server");
    expect(host.querySelector(".task-assignee-current a")).toBeNull();
    expect(host.querySelector(".task-assign-open").textContent.replace(/\s+/g, " ").trim()).toBe("spa-flaky-tests · Fix drag");
    expect(host.querySelector(".task-links a[href*='workspace']")).toBeNull();
  });

  it("uses Agent 1 when the captured agent has no name", async () => {
    const task = structuredClone(answer.result.task);
    task.identities[AGENT].name = null;
    await writeTaskRecord(DEVICE, PROJECT, answer.params.task_id,
      taskRecord(task, answer.result.timeline));
    const host = await renderCached();
    expect(host.querySelector(".task-comment .task-entry-head strong").textContent).toBe("spa-flaky-tests · Agent 1");
  });

  it("links the project agent to the project and keeps its square mark", async () => {
    const task = structuredClone(answer.result.task);
    task.assignee = { kind: "agent", agent_id: "project-01M2SCB" };
    const timeline = [{ type: "comment", id: "tc-project", author: task.assignee, body: "Ready.", created_at: "2026-09-19T10:12:00Z" }];
    await writeTaskRecord(DEVICE, PROJECT, answer.params.task_id, taskRecord(task, timeline));
    const host = await renderCached();
    expect(host.querySelector(".task-comment .task-entry-head a").textContent).toBe("Build");
    expect(host.querySelector(".task-comment .task-entry-head a").getAttribute("href")).toContain("/project/proj-1");
    expect(host.querySelector(".task-comment .task-avatar.is-project")).not.toBeNull();
  });

  it("names a deleted project agent without an agent route", async () => {
    const task = structuredClone(answer.result.task);
    const agent = { kind: "agent", agent_id: "project-01M2SCB" };
    task.identities[agent.agent_id] = { agent_id: agent.agent_id, name: null, ordinal: 1,
      workspace_id: null, workspace_name: null, provider: "claude_adk", available: false };
    task.assignee = agent;
    await writeTaskRecord(DEVICE, PROJECT, answer.params.task_id, taskRecord(task, [
      { type: "comment", id: "tc-project-gone", author: agent, body: "Ready.", created_at: "2026-09-19T10:12:00Z" },
    ]));
    const host = await renderCached();
    expect(host.querySelector(".task-comment .task-entry-head strong").textContent).toBe("Build");
    expect(host.querySelector(".task-comment .task-entry-head a")).toBeNull();
    expect(host.querySelector(".task-assignee-current a")).toBeNull();
  });

  it("links the actor, assignment target, dispatch target, assignee and workspace", async () => {
    const task = structuredClone(answer.result.task);
    const agent = { kind: "agent", agent_id: AGENT };
    const timeline = [
      { type: "event", id: "assigned", kind: "assigned", actor: agent, payload: { assignee: agent } },
      { type: "event", id: "dispatched", kind: "dispatched", actor: { kind: "user" }, payload: { agent_id: AGENT } },
      { type: "event", id: "linked", kind: "linked", actor: { kind: "user" }, payload: { workspace_id: workspace.id } },
    ];
    await writeTaskRecord(DEVICE, PROJECT, answer.params.task_id, taskRecord(task, timeline));
    const host = await renderCached();
    expect(host.querySelectorAll(".task-event .task-actor-link[href*='agent=']")).toHaveLength(3);
    expect(host.querySelector(".task-assignee-current a[href*='agent=']")).not.toBeNull();
    expect(host.querySelector(".task-event:last-child a[href*='workspace']")).not.toBeNull();
    expect(host.querySelector(".task-links a[href*='workspace']")).not.toBeNull();
  });

  it("names an unwatched notice actor from its wire identity and draws the harness", () => {
    const identity = answer.result.task.identities[AGENT];
    const html = taskNoticeLineHtml({
      task_id: answer.params.task_id, number: 102, title: "A test", action: "commented",
      actor: { kind: "agent", agent_id: AGENT, identity },
    }, { place, projectName: "Build" });
    const host = document.createElement("div");
    host.innerHTML = html;
    expect(host.querySelector(".thread-task-by").textContent).toContain("spa-flaky-tests · Fix drag");
    expect(host.querySelector(".rail-harness-icon")?.dataset.harnessIcon).toBe("codex_app_server");
    expect(host.querySelector(".thread-task-by a[href*='agent=']")).not.toBeNull();
    expect(host.querySelector("a.thread-task-number[href*='tasks']")).not.toBeNull();
  });

  it("names an unwatched notice assignee and draws its harness", () => {
    const identity = answer.result.task.identities[AGENT];
    const html = taskNoticeLineHtml({
      task_id: answer.params.task_id, number: 102, title: "A test", action: "assigned",
      actor: { kind: "user" }, assignee: { kind: "agent", agent_id: AGENT }, assignee_identity: identity,
    }, { place, projectName: "Build" });
    const host = document.createElement("div");
    host.innerHTML = html;
    expect(host.querySelector(".thread-task-said").textContent.replace(/\s+/g, " ").trim()).toBe("assigned to spa-flaky-tests · Fix drag");
    expect(host.querySelector(".thread-task-said .rail-harness-icon")?.dataset.harnessIcon).toBe("codex_app_server");
    expect(host.querySelector(".thread-task-said a[href*='agent=']")).not.toBeNull();
  });

  it("keeps a deleted notice agent as text while its task number still opens", () => {
    const identity = answer.result.task.identities[AGENT];
    const host = document.createElement("div");
    host.innerHTML = taskNoticeLineHtml({
      task_id: answer.params.task_id, number: 102, action: "assigned",
      actor: { kind: "agent", agent_id: AGENT, identity },
      assignee: { kind: "agent", agent_id: AGENT }, assignee_identity: identity,
    }, { place, projectName: "Build", workspaces: [] });
    expect(host.querySelector("a[href*='agent=']")).toBeNull();
    expect(host.querySelector("a.thread-task-number[href*='tasks']")).not.toBeNull();
    expect(host.querySelector(".thread-task-by").textContent).toContain("spa-flaky-tests · Fix drag");
  });

  it("uses the task identity on list rows and board cards without a watched digest", () => {
    for (const html of [taskRowHtml, taskCardHtml]) {
      const host = document.createElement("div");
      host.innerHTML = html(answer.result.task, { ...place, href: () => "#/task", projectName: "Build", columns: [] });
      expect(host.querySelector(".task-assignee").textContent.replace(/\s+/g, " ").trim()).toBe("spa-flaky-tests · Fix drag");
      expect(host.querySelector(".task-assignee .rail-harness-icon")?.dataset.harnessIcon).toBe("codex_app_server");
      expect(host.querySelector(".task-assignee-link[href*='agent=']")).not.toBeNull();
      expect(host.querySelector(".task-assign[data-task-assign]")).not.toBeNull();
    }
  });
});
