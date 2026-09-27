// @vitest-environment jsdom
// Who carries the primitive. Deleting a pane's own width without giving it
// .pane-col does not make the geometry consistent — it makes the pane
// full-bleed. paneLayout.test.js holds the other half: that the sheet lets no
// pane state a width of its own again.

import { describe, expect, it, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { threadHtml } from "../src/core/thread.js";
import { mountTaskView } from "../src/core/taskView.js";
import { mountGitPane } from "../src/core/gitPane.js";
import { renderFilesTab } from "../src/views/files.js";
import { wipeCache } from "../src/core/localCache.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

describe("the panes the surfaces paint carry .pane-col", () => {
  beforeEach(async () => {
    await wipeCache();
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

  // The task view is two columns, not one, so its primitive is the split the
  // Changes and Files surfaces use — same width, same gutters, same drawer.
  it("gives the task view the two-column primitive", async () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const view = mountTaskView(host, {
      taskId: "task-1",
      callRpc: async (method) => {
        if (method === "task.get")
          return { task_id: "task-1", plan_id: "task-1", goal: "Ship", state: "plan_review", docs_available: true };
        if (method === "task.stages") return { stages: [{ id: "s1", title: "Wire", state: "planned", comments: [] }] };
        return { stage_id: "s1", contents: "# Wire" };
      },
    });
    for (let i = 0; i < 80 && !host.querySelector(".ivsplit"); i++)
      await new Promise((done) => setTimeout(done, 0));
    const split = host.querySelector(".ivsplit");
    expect(split.classList.contains("pane-split")).toBe(true);
    expect(split.querySelector(".ivstages").classList.contains("pane-list")).toBe(true);
    expect(host.querySelectorAll(".pane-col")).toHaveLength(0);
    view.dispose();
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
