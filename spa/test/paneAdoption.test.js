// @vitest-environment jsdom
// Who carries the primitive. Deleting a pane's own width without giving it
// .pane-col does not make the geometry consistent — it makes the pane
// full-bleed. paneLayout.test.js holds the other half: that the sheet lets no
// pane state a width of its own again.

import { describe, expect, it, beforeEach } from "vitest";
import { threadHtml } from "../src/core/thread.js";
import { planReviewSkeletonHtml } from "../src/core/planReview.js";
import { stageBoardHtml } from "../src/views/stages.js";
import { mountProjectClusterTab } from "../src/core/projectCluster.js";

describe("the panes the surfaces paint carry .pane-col", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  const paneRoot = (html) => {
    const host = document.createElement("div");
    host.innerHTML = html;
    return host;
  };

  it("gives the conversation the primitive, full or empty", () => {
    for (const thread of [{ items: [] }, { items: [{ type: "message", data: { role: "user", body: "hi" } }] }]) {
      const host = paneRoot(threadHtml(thread));
      expect(host.firstElementChild.classList.contains("review-thread")).toBe(true);
      expect(host.firstElementChild.classList.contains("pane-col")).toBe(true);
      expect(host.querySelectorAll(".pane-col")).toHaveLength(1);
    }
  });

  it("wraps the issue's plan/stage pane in the primitive", () => {
    const host = paneRoot(planReviewSkeletonHtml());
    const pane = host.firstElementChild;
    expect(pane.classList.contains("pane-col")).toBe(true);
    // The pane's contents are unchanged — the wrapper is geometry only.
    expect(pane.querySelector("#planmeta")).toBeTruthy();
    expect(pane.querySelector("#plansummary")).toBeTruthy();
    expect(pane.querySelector("#planbody")).toBeTruthy();
    expect(host.querySelectorAll(".pane-col")).toHaveLength(1);
  });

  it("wraps the run's stage board in the primitive", () => {
    const host = paneRoot(stageBoardHtml({ state: "stage_gate" }, { stages: [], auto_advance: false }));
    const pane = host.firstElementChild;
    expect(pane.classList.contains("pane-col")).toBe(true);
    expect(pane.querySelector("#stagelist")).toBeTruthy();
    expect(pane.querySelector("#stageaction")).toBeTruthy();
    expect(host.querySelectorAll(".pane-col")).toHaveLength(1);
  });

  it.each(["inbox", "issues", "archive"])("gives the %s cluster pane the primitive", (tabId) => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const pane = mountProjectClusterTab(host, tabId, {
      projectId: "proj-1",
      callRpc: () => new Promise(() => {}),
      navigate: () => {},
    });
    const root = host.querySelector(".cluster-pane");
    expect(root.classList.contains("pane-col")).toBe(true);
    expect(host.querySelectorAll(".pane-col")).toHaveLength(1);
    pane.dispose();
  });
});
