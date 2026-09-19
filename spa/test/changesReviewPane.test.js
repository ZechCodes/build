// @vitest-environment jsdom
// The Changes surface with a review plug in it — which is where the surface
// OPENS. The rail's top row is the aggregate, so this is the first thing a
// reviewer sees, and every other test of this pane mounts without one.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { mountGitPane } = await import("../src/core/gitPane.js");
const { createReviewPlug } = await import("../src/core/changesReview.js");
const { scopeFor } = await import("../src/core/cacheScope.js");
const { wipeCache, writeCached } = await import("../src/core/localCache.js");
const { worktreeOf } = await import("./gitWireFixture.js");

const tree = worktreeOf({ "src/a.js": "new line", "uv.lock": "locked" });
const PATCH = `diff --git a/src/a.js b/src/a.js
index 1111111..2222222 100644
--- a/src/a.js
+++ b/src/a.js
@@ -1,1 +1,1 @@
-old
+new
`;

const settle = async () => {
  for (let i = 0; i < 20; i++) await new Promise((done) => setTimeout(done, 0));
};

const click = async (element) => {
  element.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
  await settle();
};

let errors = [];

async function mount({ clean = false, deviceId = "dev-1" } = {}) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const calls = [];
  const callRpc = async (method, params) => {
    calls.push({ method, params });
    if (method === "git.status")
      return clean ? tree.status({ files: [], stat: { files_changed: 0, insertions: 0, deletions: 0 } }) : tree.status();
    if (method === "git.diff") return tree.diff(params);
    if (method === "git.log") return { branch: "main", commits: [], more: false };
    return {};
  };
  const review = createReviewPlug({
    cacheScope: scopeFor(deviceId),
    fetchDiff: async () => ({ patch: PATCH, commentable: true }),
    submit: async () => {},
    renderIdleActions: (actions) => {
      actions.innerHTML = '<button class="btn mini mergeverb">Merge</button>';
      return true;
    },
  });
  // Spread, exactly as views/branchView.js does. A hand-written subset here
  // would test a wrapper the app does not have — which is how the real one
  // dropped four methods and took the toolbar down.
  const plug = { ...review, getBase: () => "main" };
  const pane = mountGitPane(container, { scope: { run_id: "run-1" }, callRpc, cacheScope: scopeFor(deviceId), review: plug });
  await settle();
  return { container, pane, calls };
}

beforeEach(async () => {
  await wipeCache();
  document.body.innerHTML = "";
  errors = [];
  window.addEventListener("error", (event) => errors.push(event.message));
  vi.spyOn(console, "error").mockImplementation((...args) => errors.push(String(args[0])));
});

afterEach(() => {
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("the Changes surface, opened on its review aggregate", () => {
  // Both halves of this surface — the pane and the plug in its rail — file what
  // they read under the machine the view handed them, and no other.
  it("caches under the cacheScope it is handed", async () => {
    const { readCached, wipeCache } = await import("../src/core/localCache.js");
    await wipeCache();

    const { pane } = await mount({ deviceId: "dev-2" });

    expect((await readCached({ deviceId: "dev-2", entityId: "run-1", kind: "status" })).value.files).toHaveLength(2);
    expect(await readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" })).toBeUndefined();
    pane.dispose();
  });

  it("still mounts the git toolbar's own verbs", async () => {
    const { container, pane } = await mount();
    expect(container.querySelector(".gtfetch")).toBeTruthy();
    expect(container.querySelector(".gtpull .btn"), "Pull").toBeTruthy();
    expect(container.querySelector(".gtpush .btn"), "Push").toBeTruthy();
    expect(container.querySelector(".gtstash .btn"), "Stash").toBeTruthy();
    pane.dispose();
  });

  it("puts the plug's merge verb in that toolbar", async () => {
    const { container, pane } = await mount();
    expect(container.querySelector(".gtmerge .mergeverb")).toBeTruthy();
    pane.dispose();
  });

  it("draws the one box under the diff", async () => {
    const { container, pane } = await mount();
    const input = container.querySelector(".gp-commit .csinput");
    expect(input).toBeTruthy();
    expect(input.rows).toBe(1);
    expect(container.querySelector(".csbox-actions .btn:not(.caret)").textContent.trim()).toBe("Comment");
    pane.dispose();
  });

  it("raises nothing to the console doing any of it", async () => {
    const { pane } = await mount();
    expect(errors).toEqual([]);
    pane.dispose();
  });

  // The state the surface is usually in: the work is committed, so there is
  // nothing to commit and the box can only comment.
  it("still mounts the toolbar's verbs over a committed tree", async () => {
    const { container, pane } = await mount({ clean: true });
    expect(errors).toEqual([]);
    expect(container.querySelector(".gtpull .btn"), "Pull").toBeTruthy();
    expect(container.querySelector(".gtstash .btn"), "Stash").toBeTruthy();
    expect(container.querySelector(".gtmerge .mergeverb"), "Merge").toBeTruthy();
    pane.dispose();
  });

  it("still offers the box to comment in over a committed tree", async () => {
    const { container, pane } = await mount({ clean: true });
    expect(container.querySelector(".gp-commit .csinput")).toBeTruthy();
    pane.dispose();
  });

  // The bar is the surface's and the mark is made inside the plug, so the plug
  // has to say so — otherwise the verbs the selection raises never appear.
  it("raises the selection's verbs in the bar above the stack", async () => {
    const { container, pane } = await mount();
    expect(container.querySelector(".gittoolbar .selbar")).toBe(null);
    container.querySelector(".fselect-box").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    expect(container.querySelector(".gittoolbar .selcount").textContent).toBe("1 file selected");
    expect(container.querySelector(".cdetail-host .selbar"), "and not over the diffs").toBe(null);
    pane.dispose();
  });

  // The reviewer's bug: Clear emptied the selection and took the bar away, but
  // every box in the diff stayed ticked — the render said unticked, and a box's
  // ticked-ness is a property no attribute comparison ever reached.
  it("unticks every box when the selection is cleared", async () => {
    const { container, pane } = await mount();
    container.querySelector(".fselect-box").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    expect(container.querySelector(".fselect-box").checked).toBe(true);

    container.querySelector(".selclear").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    expect(container.querySelector(".gittoolbar .selbar")).toBe(null);
    expect(container.querySelector(".fselect-box").checked, "the box the reader ticked").toBe(false);
    pane.dispose();
  });

  it("unticks them after Approve all takes the selection too", async () => {
    const { container, pane } = await mount();
    container.querySelector(".fselect-box").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    container.querySelector(".selapprove").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    expect(container.querySelector(".fselect-box").checked).toBe(false);
    expect(container.querySelector(".fapprove").getAttribute("aria-pressed")).toBe("true");
    pane.dispose();
  });

  // The reviewer's bug: the box selects the file AND expanded its diff, because
  // a capped file treats any click in it as "show me the rest".
  it("selects a file without expanding it", async () => {
    const { container, pane } = await mount();
    const file = container.querySelector(".file");
    expect(file.classList.contains("capped")).toBe(true);
    // A real press, and only that: activating a checkbox flips it and fires its
    // own change event, so a test that also sets `checked` and dispatches one
    // is pressing it twice.
    const box = file.querySelector(".fselect-box");
    box.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await settle();
    const after = container.querySelector(".file");
    expect(after.classList.contains("capped"), "the diff stayed folded").toBe(true);
    expect(after.querySelector(".fselect-box").checked, "and the file is selected").toBe(true);
    pane.dispose();
  });
});

// The aggregate the surface opens on is the `diff` record. The surface is
// asked for one only where the cache holds none.
describe("the aggregate over a filled cache", () => {
  async function mountOverDiff({ held = null } = {}) {
    if (held) await writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" }, held);
    const container = document.createElement("div");
    document.body.appendChild(container);
    const callRpc = async (method, params) => {
      if (method === "git.status") return tree.status();
      if (method === "git.diff") return tree.diff(params);
      if (method === "git.log") return { branch: "main", commits: [], more: false };
      return {};
    };
    const fetchDiff = vi.fn(async () => ({ patch: PATCH.replace("+new", "+off the wire"), commentable: true }));
    const review = createReviewPlug({ cacheScope: scopeFor("dev-1"), entity: "run-1", fetchDiff, submit: async () => {} });
    const pane = mountGitPane(container, {
      scope: { run_id: "run-1" },
      callRpc,
      cacheScope: scopeFor("dev-1"),
      review: { ...review, getBase: () => "main" },
    });
    await settle();
    // The rail's top row is the aggregate; a dirty tree opens on Uncommitted.
    await click(container.querySelector('.rrow[data-sel="review"]'));
    return { container, pane, fetchDiff };
  }

  it("paints the record and asks the surface for nothing", async () => {
    const held = { patch: PATCH.replace("+new", "+from the record"), commentable: true };
    const { container, pane, fetchDiff } = await mountOverDiff({ held });
    expect(container.textContent).toContain("from the record");
    expect(fetchDiff).not.toHaveBeenCalled();
    pane.dispose();
  });

  it("asks once, and only once, when the cache holds no diff", async () => {
    const { container, pane, fetchDiff } = await mountOverDiff();
    expect(container.textContent).toContain("off the wire");
    expect(fetchDiff).toHaveBeenCalledTimes(1);
    await settle();
    expect(fetchDiff).toHaveBeenCalledTimes(1);
    pane.dispose();
  });
});

describe("workspace comments", () => {
  async function workspacePane({ fail = false } = {}) {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const calls = [];
    const callRpc = async (method, params) => {
      calls.push({ method, params });
      if (method === "git.status") return tree.status();
      if (method === "git.diff") return tree.diff(params);
      if (method === "git.log") return { branch: "main", commits: [], more: false };
      if (method === "git.unpushed") return { patch: PATCH + PATCH.replaceAll("src/a.js", "src/b.js"), base: { kind: "empty" } };
      if (method === "workspace.ensure_conversation") return { entity_id: "workspace-thread" };
      if (method === "thread.post" && fail) throw new Error("offline");
      return {};
    };
    // Where each source of this workspace is mounted is the machine's
    // workspace list's to say, and the pane reads it there.
    await writeCached(
      { deviceId: "dev-1", entityId: "", kind: "workspaces" },
      [{ id: "w", root: "/work", directories: [{ source_id: "s", path: "/work/source" }] }],
    );
    const pane = mountGitPane(container, {
      scope: { workspace_id: "w", source_id: "s" }, callRpc,
      cacheScope: scopeFor("dev-1"),
      agentSelection: { scope: () => ({ agent_id: "selected-agent" }) },
    });
    await settle();
    return { pane, container, calls };
  }

  it("offers file comments and both composer verbs, sending all files to the selected agent", async () => {
    const { pane, container, calls } = await workspacePane();
    expect(container.querySelector(".fcmt")).not.toBeNull();
    expect(container.querySelector(".csinput").placeholder).toContain("or write a commit message");
    container.querySelector(".csinput").value = "Please fix these";
    await click(container.querySelector(".csbox-actions .btn:not(.caret)"));
    const posted = calls.find(({ method }) => method === "thread.post").params;
    expect(posted.entity_id).toBe("workspace-thread");
    expect(posted.agent_id).toBe("selected-agent");
    expect(posted.messages[0]).toMatchObject({ body: "Please fix these", viewing_context: { items: [{ kind: "diff", path: "source/src/a.js", mode: "all" }, { kind: "diff", path: "source/src/b.js", mode: "all" }] } });
    expect(calls.some(({ method }) => method === "git.commit")).toBe(false);
    pane.dispose();
  });

  it("comments on selected files without narrowing the commit path logic", async () => {
    const { pane, container, calls } = await workspacePane();
    const checkbox = container.querySelector('.fselect-box');
    checkbox.checked = true;
    checkbox.dispatchEvent(new window.Event("change", { bubbles: true }));
    container.querySelector(".csinput").value = "Only this file";
    await click(container.querySelector(".csbox-actions .btn:not(.caret)"));
    const posted = calls.find(({ method }) => method === "thread.post").params;
    expect(posted.messages[0].viewing_context.items).toEqual([{ kind: "diff", path: "source/src/a.js", mode: "all" }]);
    pane.dispose();
  });

  it("sends an anchored whole-file comment without ambient context", async () => {
    const { pane, container, calls } = await workspacePane();
    await click(container.querySelector(".fcmt"));
    document.querySelector(".cp-input").value = "Split this up";
    await click(document.querySelector(".cp-save"));
    await click(container.querySelector(".csbox-actions .btn:not(.caret)"));
    const posted = calls.find(({ method }) => method === "thread.post").params;
    expect(posted.messages[0]).toMatchObject({ body: "Split this up", anchor: { artifact: "diff", path: "source/src/a.js" } });
    pane.dispose();
  });

  it("anchors selected text with ambient context disabled", async () => {
    const { pane, container, calls } = await workspacePane();
    const code = container.querySelector('tr.add .code');
    code.closest(".file").classList.remove("capped");
    const range = document.createRange();
    range.selectNodeContents(code);
    range.getBoundingClientRect = () => ({ top: 0, bottom: 10, left: 0, right: 30, width: 30, height: 10 });
    const selection = window.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    code.dispatchEvent(new window.Event("pointerup", { bubbles: true }));
    await settle();
    await click(document.querySelector(".cp-add"));
    document.querySelector(".cp-input").value = "Rename this";
    await click(document.querySelector(".cp-save"));
    selection.removeAllRanges();
    await click(container.querySelector(".csbox-actions .btn:not(.caret)"));
    const posted = calls.find(({ method }) => method === "thread.post").params;
    expect(posted.messages[0]).toMatchObject({ body: "Rename this", anchor: { path: "source/src/a.js", snippet: "new", line_start: 1, line_end: 1, side: "new" } });
    pane.dispose();
  });

  it("keeps a failed comment draft for retry", async () => {
    const { pane, container } = await workspacePane({ fail: true });
    container.querySelector(".csinput").value = "Keep my comment";
    await click(container.querySelector(".csbox-actions .btn:not(.caret)"));
    expect(container.querySelector(".csinput").value).toBe("Keep my comment");
    expect(container.querySelector(".csbox-actions .btn:not(.caret)").disabled).toBe(false);
    pane.dispose();
  });
});
