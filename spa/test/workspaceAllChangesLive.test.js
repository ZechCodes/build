// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { worktreeOf } from "./gitWireFixture.js";

const changeWatchers = vi.hoisted(() => []);

vi.mock("../src/core/changeEvents.js", () => ({
  watchChanges: (registration) => {
    const watcher = { ...registration, disposed: false };
    changeWatchers.push(watcher);
    return { dispose: () => (watcher.disposed = true) };
  },
}));

const { mountGitPane } = await import("../src/core/gitPane.js");

const patchFor = (value) => `diff --git a/src/live.js b/src/live.js
index 1111111..2222222 100644
--- a/src/live.js
+++ b/src/live.js
@@ -1,1 +1,1 @@
-before
+${value}
`;

const renamePatch = `diff --git a/src/live.js b/src/live.js
deleted file mode 100644
index 2222222..0000000
--- a/src/live.js
+++ /dev/null
@@ -1,1 +0,0 @@
-cider
diff --git a/src/renamed.js b/src/renamed.js
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/src/renamed.js
@@ -0,0 +1,1 @@
+cider
`;

const settle = async () => {
  for (let i = 0; i < 30; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};

describe("workspace All Changes live refresh", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
    changeWatchers.length = 0;
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("uses the pane's workspace watcher for filesystem news while preserving review state", async () => {
    const tree = worktreeOf({});
    const heldStatus = tree.status();
    let aggregate = { patch: "", diff_key: "all-empty", base: { kind: "push_target", label: "origin/main" } };
    const unpushedKeys = [];
    const callRpc = vi.fn(async (method, params) => {
      if (method === "git.status")
        return params.if_status_key
          ? { unchanged: true, status_key: heldStatus.status_key }
          : heldStatus;
      if (method === "git.log") return { branch: "main", commits: [], more: false };
      if (method === "git.unpushed") {
        unpushedKeys.push(params.if_diff_key);
        return params.if_diff_key === aggregate.diff_key
          ? { unchanged: true, diff_key: aggregate.diff_key }
          : aggregate;
      }
      return {};
    });
    const container = document.createElement("div");
    document.body.appendChild(container);
    const pane = mountGitPane(container, {
      scope: { workspace_id: "workspace-1", source_id: "source-1" },
      callRpc,
    });
    await settle();

    const liveWatchers = changeWatchers.filter((watcher) => !watcher.disposed);
    expect(liveWatchers).toHaveLength(1);
    const watcher = liveWatchers[0];
    expect(watcher).toMatchObject({
      // The board, not the workspace id: the bridge names no entity for a
      // workspace source, so its id on an item is a word that never comes.
      entity: null,
      kinds: ["state", "git", "files"],
      mode: "realtime",
    });
    // A workspace source is not an entity the sync layer walks, so this pane
    // reads it — but on the bridge's word, never on a clock of its own.
    expect(watcher.intervalMs).toBeUndefined();
    expect(container.textContent).toContain("No file changes yet.");

    // A filesystem invalidation updates the aggregate even though status/HEAD,
    // the commit page and line counts all remain unchanged.
    aggregate = { ...aggregate, patch: patchFor("alpha"), diff_key: "all-alpha" };
    watcher.refresh();
    await settle();
    expect(container.textContent).toContain("alpha");
    expect(callRpc.mock.calls.filter(([method]) => method === "git.status").at(-1)[1]).toMatchObject({
      if_status_key: heldStatus.status_key,
    });

    const file = () => container.querySelector('.file[data-key$="src/live.js"]');
    file().querySelector("td.code").dispatchEvent(new MouseEvent("click", { bubbles: true }));
    file().querySelector(".fselect-box").dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    const scroller = container.querySelector(".cdetail-host");
    scroller.scrollTop = 137;
    expect(file().classList.contains("capped")).toBe(false);
    expect(file().querySelector(".fselect-box").checked).toBe(true);

    aggregate = { ...aggregate, patch: patchFor("bravo"), diff_key: "all-bravo" };
    watcher.refresh();
    await settle();
    expect(container.textContent).toContain("bravo");
    expect(file().classList.contains("capped"), "the reader's open fold").toBe(false);
    expect(file().querySelector(".fselect-box").checked, "the reader's selection").toBe(true);
    expect(scroller.scrollTop, "the reader's place").toBe(137);

    const standingFile = file();
    watcher.refresh();
    await settle();
    expect(unpushedKeys.at(-1)).toBe("all-bravo");
    expect(file(), "an unchanged conditional response writes no DOM").toBe(standingFile);

    // News received during a draft stays queued, then paints as soon as the
    // draft is cleared instead of waiting for navigation or another event.
    const draft = container.querySelector(".csinput");
    draft.value = "keep this thought";
    draft.dispatchEvent(new Event("input", { bubbles: true }));
    aggregate = { ...aggregate, patch: patchFor("cider"), diff_key: "all-cider" };
    watcher.refresh();
    await settle();
    expect(container.textContent).not.toContain("cider");
    expect(draft.value).toBe("keep this thought");
    draft.value = "";
    draft.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(container.textContent).toContain("cider");

    // Git represents an unstaged filesystem rename as the old path disappearing
    // and the new path appearing; both halves arrive in the same aggregate.
    aggregate = { ...aggregate, patch: renamePatch, diff_key: "all-renamed" };
    watcher.refresh();
    await settle();
    expect([...container.querySelectorAll(".fpath")].map((path) => path.textContent.trim())).toEqual([
      "src/live.js",
      "src/renamed.js",
    ]);

    aggregate = { ...aggregate, patch: "", diff_key: "all-clean" };
    watcher.refresh();
    await settle();
    expect(container.querySelector(".file")).toBeNull();
    expect(container.textContent).toContain("No file changes yet.");
    expect(new Set(callRpc.mock.calls.filter(([method]) => method === "git.status").map(([, params]) => params.if_status_key))).toEqual(
      new Set([undefined, heldStatus.status_key]),
    );

    pane.dispose();
  });
});
