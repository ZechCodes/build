/** @vitest-environment jsdom */
import { afterEach, describe, expect, it, vi } from "vitest";
import { assignableAgents, assigneeOptions, assignParams, workspaceAgents } from "../src/core/trackerAssignee.js";
import { openAssigneePicker } from "../src/core/trackerAssigneePicker.js";
import { actorName } from "../src/core/trackerLineWords.js";
import { taskAvatarHtml } from "../src/core/taskAvatar.js";
import { actorHref } from "../src/core/trackerIdentity.js";
import { referenceTables, resolverFor } from "../src/core/referenceTargets.js";
import { called, painted } from "./waits.js";

const projectKey = "dev/proj-1";
const worker = { id: "project-worker", name: "Rail builder", provider: "codex", project_role: "worker" };
const pin = { id: "project-pin", name: "Coordinator", project_role: "coordinator" };
const project = { id: "proj-1", projectKey, name: "Build", projectRoster: {
  entity_id: "owner", pinned_agent_id: pin.id, agents: [worker, pin],
}, assignProjectAgent: true };
const feed = { projects: [project], workspaces: [], items: [] };
let picker;
afterEach(async () => { await picker?.close(); picker = null; document.body.innerHTML = ""; });

describe("cached project worker assignees", () => {
  it("offers workers only in their exact project's separate group", () => {
    expect(workspaceAgents(feed, projectKey)).toEqual([]);
    const groups = assignableAgents(feed, projectKey);
    expect(groups[0].kind).toBe("project");
    expect(groups[0].agents.map((agent) => agent.id)).toEqual([worker.id]);
    const options = assigneeOptions(groups);
    expect(options.find((one) => one.id === `agent:${worker.id}`).label).toBe("Rail builder");
    expect(options.filter((one) => one.kind === "new_agent")).toEqual([]);
    expect(options.find((one) => one.kind === "new_project_agent")).toBeTruthy();
    expect(assignableAgents(feed, "other/proj-1")).toEqual([]);
  });
  it("keeps legacy rosters pinned-only and gates creation independently", () => {
    expect(assigneeOptions(assignableAgents({ ...feed, projects: [{ ...project, projectRoster: undefined }] }, projectKey)))
      .not.toEqual(expect.arrayContaining([expect.objectContaining({ kind: "new_project_agent" })]));
    const options = assigneeOptions(assignableAgents({ ...feed, projects: [{ ...project, assignProjectAgent: false }] }, projectKey));
    expect(options.some((one) => one.kind === "agent")).toBe(true);
    expect(options.some((one) => one.kind === "new_project_agent")).toBe(false);
  });
  it("uses the worker name key and only adds operation IDs to supported creation", () => {
    const option = { kind: "new_project_agent" };
    expect(assignParams("task", option, { name: " Ada ", choice: { provider: "codex" }, operationId: "op-1" }))
      .toEqual({ task_id: "task", assignee: { kind: "new_project_agent", agent_name: "Ada", provider: "codex" }, operation_id: "op-1" });
    expect(assignParams("task", { kind: "user" }, { operationId: "op-1" })).not.toHaveProperty("operation_id");
  });
  it("keeps a creation operation ID across a failed dispatch retry and preserves the worker name", async () => {
    const callRpc = vi.fn().mockRejectedValueOnce(new Error("Try again")).mockResolvedValue({ task: {} });
    picker = openAssigneePicker({ task: { id: "task", number: 1 }, options: assigneeOptions(assignableAgents(feed, projectKey)),
      current: "new_project_agent", callRpc });
    const name = document.querySelector("#task-assign-worker");
    expect(name).not.toBeNull();
    name.value = "Ada";
    name.dispatchEvent(new Event("input"));
    document.querySelector("[data-assign-go]").click();
    await painted(() => !document.querySelector("[data-assign-go]").disabled);
    expect(document.querySelector("#task-assign-worker").value).toBe("Ada");
    document.querySelector("[data-assign-go]").click();
    await called(callRpc, (mock) => mock.mock.calls.length === 2);
    const first = callRpc.mock.calls[0][1];
    expect(first.assignee.agent_name).toBe("Ada");
    expect(first.operation_id).toEqual(expect.any(String));
    expect(callRpc.mock.calls[1][1].operation_id).toBe(first.operation_id);
  });
  it("reconciles a late cached roster without replacing the selection with another agent", async () => {
    const callRpc = vi.fn();
    picker = openAssigneePicker({ task: { id: "task", number: 1 }, options: assigneeOptions(assignableAgents(feed, projectKey)),
      current: `agent:${worker.id}`, callRpc });
    picker.setOptions(assigneeOptions(assignableAgents({ ...feed, projects: [{ ...project, projectRoster: {
      ...project.projectRoster, agents: [pin],
    } }] }, projectKey)));
    document.querySelector("[data-assign-go]").click();
    await painted(() => document.querySelector(".create-error").textContent);
    expect(callRpc).not.toHaveBeenCalled();
    expect(document.querySelector(".create-error").textContent).toContain("no longer available");
  });
});

describe("project worker identities", () => {
  const actor = { kind: "agent", agent_id: worker.id };
  const context = { deviceId: "dev", projectId: "proj-1", projectName: "Build", agentGroups: [{ kind: "project", agents: [worker] }],
    agentLabels: { [worker.id]: worker.name }, agentProviders: { [worker.id]: worker.provider } };
  it("renders the worker's own name and harness while preserving coordinator fallback", () => {
    expect(actorName(actor, context)).toBe("Rail builder");
    expect(actorName(worker.id, context)).toBe("Rail builder");
    expect(actorName({ kind: "agent", agent_id: pin.id }, context)).toBe("Build");
    expect(taskAvatarHtml(actor, context)).toContain("is-harness");
    expect(actorHref(actor, context)).toContain("agent=project-worker");
  });
  it("uses an inline immutable worker identity before the legacy ID prefix", () => {
    const inline = { ...actor, identity: { name: "Ada", provider: "codex", available: true } };
    expect(actorName(inline, { projectName: "Build" })).toBe("Ada");
    expect(taskAvatarHtml(inline, { projectName: "Build" })).toContain("is-harness");
  });
  it("retains an unavailable worker's durable name but removes its link", () => {
    const reading = { ...context, agentLabels: {}, identities: { [worker.id]: { name: "Ada", provider: "codex", available: false } } };
    expect(actorName(actor, reading)).toBe("Ada");
    expect(actorHref(actor, reading)).toBe("");
  });
  it("trusts the complete current roster over a stale available identity", () => {
    const reading = { ...context, agentGroups: [{ kind: "project", pinnedAgentId: pin.id, agents: [] }],
      identities: { [worker.id]: { name: "Ada", available: true } } };
    expect(actorHref(actor, reading)).toBe("");
    expect(actorHref({ kind: "agent", agent_id: "project-foreign" }, reading)).toBe("");
    expect(actorHref({ kind: "agent", agent_id: pin.id }, reading)).toContain("agent=project-pin");
  });
  it("resolves exact workers without a workspace and rejects foreign IDs in a complete roster", () => {
    const resolve = resolverFor(referenceTables({ feed }), { place: { deviceId: "dev", projectId: "proj-1" } });
    expect(resolve.agent(worker.id)).toMatchObject({ agentId: worker.id, name: "Rail builder", deviceId: "dev", projectId: "proj-1" });
    expect(resolve.agent("project-foreign")).toBeNull();
  });
  it("keeps same project and agent IDs on two devices separate", () => {
    const other = { ...project, projectKey: "other/proj-1", projectRoster: { ...project.projectRoster,
      agents: [{ ...worker, name: "Other worker" }, pin] } };
    const tables = referenceTables({ feed: { ...feed, projects: [project, other] } });
    expect(resolverFor(tables, { place: { deviceId: "dev", projectId: "proj-1" } }).agent(worker.id).name).toBe("Rail builder");
    expect(resolverFor(tables, { place: { deviceId: "other", projectId: "proj-1" } }).agent(worker.id).name).toBe("Other worker");
  });
});
