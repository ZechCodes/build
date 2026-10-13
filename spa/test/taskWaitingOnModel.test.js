import { describe, expect, it } from "vitest";
import { blockerChoices, removalConsequence, waitingOnNotice } from "../src/core/taskWaitingOnModel.js";
import { dependencyRowsHtml, waitingOnFrameHtml } from "../src/core/taskWaitingOnRender.js";

const edge = (id, resolved = false) => ({ task_id: id, number: 12, title: '<img src=x onerror="boom">', resolved, resolved_by: resolved ? "close" : null });
const task = (extra = {}) => ({ id: "current", project_id: "p", assignee: { kind: "agent", agent_id: "a" }, dependencies: { blocked: true, waiting_on: [edge("b")] }, ...extra });
describe("task-page dependency choices and consequences", () => {
  it("offers only same-project unlinked tasks, and searches number and title", () => {
    const rows = [{ id: "current", project_id: "p" }, { id: "b", project_id: "p" }, { id: "other", project_id: "q" }, { id: "new", project_id: "p", number: 42, title: "Ship it" }];
    expect(blockerChoices(task(), rows, "p", "42").map((row) => row.id)).toEqual(["new"]);
    expect(blockerChoices(task(), rows, "p", "ship").map((row) => row.id)).toEqual(["new"]);
  });
  it("only promises release of a saved waiting/queued assignment on the last unresolved edge", () => {
    for (const state of ["waiting", "queued"]) expect(removalConsequence(task({ deferred_assignment: { id: "intent", state } }), "b")).toContain("starts the saved assignment");
    for (const deferred_assignment of [null, { state: "failed" }, { state: "cancelled" }, { state: "consumed" }]) expect(removalConsequence(task({ deferred_assignment }), "b")).not.toContain("starts");
    expect(removalConsequence(task({ assignee: { kind: "user" } }), "b")).not.toContain("starts");
    const many = task({ deferred_assignment: { state: "waiting" }, dependencies: { waiting_on: [edge("b"), edge("c")] } });
    expect(removalConsequence(many, "b")).not.toContain("starts");
  });
  it("explains consumed work without promising interruption or automatic redispatch", () => {
    expect(waitingOnNotice(task({ status: "in_progress", deferred_assignment: null }))).toContain("does not stop a running turn");
    expect(waitingOnNotice(task({ status: "in_progress", deferred_assignment: null }))).toContain("Assign again");
    expect(waitingOnNotice(task({ deferred_assignment: { state: "waiting" } }))).toContain("Done or closed");
  });
  it("renders escaped task links and distinguishes closed resolution from Done", () => {
    const html = dependencyRowsHtml([edge("b", true), { ...edge("c", true), resolved_by: "done" }], { deviceId: "d", projectId: "p" }, true);
    expect(html).toContain("Closed · resolved");
    expect(html).toContain("Done · resolved");
    expect(html).toContain("&lt;img");
    expect(html).not.toContain("<img");
    expect(html).toContain("/tasks/b");
    expect(html).toContain('data-waiting-remove="b"');
    expect(waitingOnFrameHtml()).toContain('data-waiting-picker');
  });
});
