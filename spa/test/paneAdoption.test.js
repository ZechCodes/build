// @vitest-environment jsdom
// Who carries the primitive. Deleting a pane's own width without giving it
// .pane-col does not make the geometry consistent — it makes the pane
// full-bleed. paneLayout.test.js holds the other half: that the sheet lets no
// pane state a width of its own again.

import { describe, expect, it, beforeEach, vi } from "vitest";
import { threadHtml } from "../src/core/thread.js";
import { planReviewSkeletonHtml } from "../src/core/planReview.js";
import { stageBoardHtml } from "../src/views/stages.js";
import { mountProjectClusterTab } from "../src/core/projectCluster.js";
import { mountGitPane } from "../src/core/gitPane.js";
import { renderFilesTab } from "../src/views/files.js";

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

// The other layout. Changes and Files are the only two-column tabs, and .flush
// used to leave them edge-to-edge — the one geometry on the shell that answered
// to nothing. The primitive goes on the split itself, not on a wrapper, so the
// rail and the detail column stay siblings of one flex row and keep their own
// scroll.
describe("the two-column panes carry .pane-split", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("gives the Changes pane the primitive over its rail and detail column", async () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const pane = mountGitPane(host, {
      scope: { run_id: "run-1" },
      callRpc: vi.fn(async (method) =>
        method === "git.status"
          ? { branch: "main", path: "/repo", head: "f".repeat(40), files: [], stat: null, patch: "", truncated: false }
          : { branch: "main", commits: [], more: false },
      ),
    });
    await vi.waitFor(() => expect(host.querySelector(".changes2")).toBeTruthy());
    const split = host.querySelector(".changes2");
    expect(split.classList.contains("pane-split")).toBe(true);
    // Geometry only: the split still holds the same two self-scrolling columns.
    expect(split.querySelector(".crail-host")).toBeTruthy();
    expect(split.querySelector(".cdetail-host")).toBeTruthy();
    expect(host.querySelectorAll(".pane-split")).toHaveLength(1);
    pane.dispose();
  });

  it("gives the Files browser the primitive over its tree and preview", () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    renderFilesTab(host, { scope: { run_id: "run-1" }, callRpc: () => new Promise(() => {}) });
    const split = host.querySelector(".files");
    expect(split.classList.contains("pane-split")).toBe(true);
    expect(split.querySelector("#ftree")).toBeTruthy();
    expect(split.querySelector("#fpreview")).toBeTruthy();
    expect(host.querySelectorAll(".pane-split")).toHaveLength(1);
  });
});
