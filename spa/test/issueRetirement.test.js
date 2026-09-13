import { describe, expect, it } from "vitest";
import { manualRoute } from "../src/core/compose.js";
import { captureDecisionHtml, captureDecisionModel, manualRouteParams } from "../src/core/captureDecision.js";
import { captureRowHtml } from "../src/core/inbox.js";

describe("retired issue creation entry points", () => {
  it("turns legacy manual compose drafts into branch dispatches", () => {
    expect(manualRoute({ kind: "issue", projectId: "p1", text: "Fix it" })).toEqual({
      method: "branch.dispatch", params: { project_id: "p1", instruction: "Fix it" },
    });
  });

  it("offers only branch routing on the capture decision page", () => {
    const model = captureDecisionModel({
      id: "c1", text: "Fix it", state: "routing",
      question: { text: "Where?", options: [
        { id: "issue", label: "File issue", project_id: "p1", kind: "issue" },
        { id: "branch", label: "Use branch", project_id: "p1", kind: "branch" },
      ] },
    }, { projects: [{ id: "p1", name: "Project" }] });
    expect(model.options.map((option) => option.id)).toEqual(["branch"]);
    const html = captureDecisionHtml(model, { projects: [{ id: "p1", name: "Project" }], projectId: "p1", branch: "" });
    expect(html).not.toContain("Issue");
    expect(html).toContain('data-capture-kind="branch"');
    expect(manualRouteParams("c1", { projectId: "p1", kind: "issue" })).toEqual({ capture_id: "c1", project_id: "p1", kind: "branch" });
  });

  it("offers only Branch in the inbox capture reroute menu", () => {
    const html = captureRowHtml({
      key: "capture:c1", kind: "capture", captureId: "c1", title: "Fix it", captureState: "routed", routedTo: { project: "Project", kind: "issue" }, route: null,
    }, { rerouteKey: "capture:c1", projects: [{ id: "p1", name: "Project" }] });
    expect(html).not.toContain('data-reroute-kind="issue"');
    expect(html).toContain('data-reroute-branch-open="p1"');
  });
});
