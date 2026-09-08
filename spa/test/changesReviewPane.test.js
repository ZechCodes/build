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
const { setCacheDevice } = await import("../src/core/cacheScope.js");
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
  for (let i = 0; i < 10; i++) await Promise.resolve();
  await new Promise((done) => setTimeout(done, 0));
};

const click = async (element) => {
  element.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
  await settle();
};

let errors = [];

async function mount({ clean = false } = {}) {
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
    fetchDiff: async () => ({ patch: PATCH, commentable: true }),
    submit: async () => {},
    renderIdleActions: (actions) => {
      actions.innerHTML = '<button class="btn mini mergeverb">Merge</button>';
      return true;
    },
  });
  const plug = {
    getBase: () => "main",
    mount: (element, options) => review.mount(element, options),
    unmount: () => review.unmount(),
    refreshActions: () => review.refreshActions(),
    commentOffer: () => review.commentOffer(),
    sendComments: () => review.sendComments(),
  };
  const pane = mountGitPane(container, { scope: { run_id: "run-1" }, callRpc, review: plug });
  await settle();
  return { container, pane, calls };
}

beforeEach(() => {
  document.body.innerHTML = "";
  setCacheDevice("dev-1");
  errors = [];
  window.addEventListener("error", (event) => errors.push(event.message));
  vi.spyOn(console, "error").mockImplementation((...args) => errors.push(String(args[0])));
});

afterEach(() => {
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("the Changes surface, opened on its review aggregate", () => {
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
