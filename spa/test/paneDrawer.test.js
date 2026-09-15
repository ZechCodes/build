// @vitest-environment jsdom
// The two-column layout on a phone. Below the stacking width the list column
// (commit rail / file tree) is not a column beside the detail any more — it is
// a drawer that drops in from the top of the pane over it, opened and shut by
// a trigger row that names what the detail column is showing.
// paneLayout.test.js holds the CSS half of this; here is the behaviour: what
// opens it, what closes it, what the trigger says, and what the two panes that
// use it hand the primitive.

import { describe, expect, it, beforeEach, vi } from "vitest";
import { initPaneDrawer, paneDrawerHtml } from "../src/core/paneDrawer.js";
import { mountGitPane } from "../src/core/gitPane.js";
import { renderFilesTab } from "../src/views/files.js";

/** A split with a list column and a detail column, wired the way both real
 *  panes wire it. */
function mountSplit({ closeOnSelect = ".pick", summary } = {}) {
  const split = document.createElement("div");
  split.className = "pane-split";
  split.innerHTML = `<div class="pane-list" id="list"><div class="pick" id="one">one</div><div class="move" id="deeper">deeper</div></div><div class="detail"></div>${paneDrawerHtml("commits")}`;
  document.body.appendChild(split);
  const drawer = initPaneDrawer(split, { list: split.querySelector("#list"), closeOnSelect, summary });
  return {
    split,
    drawer,
    handle: split.querySelector("[data-pane-handle]"),
    scrim: split.querySelector("[data-pane-scrim]"),
    says: () => split.querySelector("[data-pane-summary]").textContent,
  };
}

const isOpen = (split) => split.classList.contains("drawer-open");
const pressEscape = () => document.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Escape", bubbles: true }));

describe("the pane drawer", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("mounts closed, with a handle that says what it opens", () => {
    const { split, handle, scrim } = mountSplit();
    expect(isOpen(split)).toBe(false);
    expect(scrim).toBeTruthy();
    expect(handle.getAttribute("aria-expanded")).toBe("false");
    expect(handle.getAttribute("aria-label")).toBe("Show commits");
    // The handle names the column it moves, so a screen reader is told what
    // opened rather than just that something did.
    expect(handle.getAttribute("aria-controls")).toBe("list");
  });

  it("names what the detail column is showing, and repaints on request", () => {
    let open = "Uncommitted · clean";
    const { says, drawer } = mountSplit({ summary: () => open });
    expect(says()).toBe("Uncommitted · clean");
    // The pane owns the selection, so the pane says when the words moved: a
    // click is heard here before the pane has acted on it.
    open = "Add the drawer · 1a2b3c4";
    drawer.refresh();
    expect(says()).toBe("Add the drawer · 1a2b3c4");
  });

  it("falls back to the column's own name when the pane offers no words", () => {
    expect(mountSplit().says()).toBe("commits");
  });

  it("pulls the drawer out and pushes it back from the one handle", () => {
    const { split, handle } = mountSplit();
    handle.click();
    expect(isOpen(split)).toBe(true);
    expect(handle.getAttribute("aria-expanded")).toBe("true");
    expect(handle.getAttribute("aria-label")).toBe("Hide commits");
    handle.click();
    expect(isOpen(split)).toBe(false);
    expect(handle.getAttribute("aria-label")).toBe("Show commits");
  });

  it("closes on the scrim and on Escape, the way the project rail does", () => {
    const { split, handle, scrim } = mountSplit();
    handle.click();
    scrim.click();
    expect(isOpen(split)).toBe(false);
    handle.click();
    pressEscape();
    expect(isOpen(split)).toBe(false);
  });

  it("closes on a row that fills the detail column, not on one that only moves the list", () => {
    // Opening the drawer is how you reach the list; picking from it is what you
    // opened it for, and what you picked is behind the drawer.
    const { split } = mountSplit();
    split.querySelector("[data-pane-handle]").click();
    split.querySelector("#deeper").click();
    expect(isOpen(split)).toBe(true);
    split.querySelector("#one").click();
    expect(isOpen(split)).toBe(false);
  });

  it("leaves no document listener behind once disposed", () => {
    const { split, handle, drawer } = mountSplit();
    handle.click();
    drawer.dispose();
    pressEscape();
    // Disposed means disposed: the class the pane was left in is the pane's own
    // business, but nothing may still be listening for a key on its behalf.
    expect(isOpen(split)).toBe(true);
    split.remove();
    pressEscape();
  });
});

describe("the panes that carry a drawer", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("hands the Changes pane's commit rail to the primitive", async () => {
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
    await vi.waitFor(() => expect(host.querySelector(".rrow")).toBeTruthy());
    const split = host.querySelector(".changes2");
    expect(split.querySelector(".crail-host").classList.contains("pane-list")).toBe(true);
    expect(split.querySelector("[data-pane-handle]")).toBeTruthy();
    split.querySelector("[data-pane-handle]").click();
    expect(isOpen(split)).toBe(true);
    // Picking what the rail is for — a set of changes to read — closes it.
    split.querySelector(".rrow").click();
    expect(isOpen(split)).toBe(false);
    // And the trigger says what it left behind the drawer.
    await vi.waitFor(() =>
      expect(split.querySelector("[data-pane-summary]").textContent).toBe("Uncommitted · clean"),
    );
    pane.dispose();
  });

  it("opens the Changes trigger on the rail's own offer", async () => {
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
    await vi.waitFor(() => expect(host.querySelector(".rrow")).toBeTruthy());
    // A clean branch with no aggregate opens at the commit list with nothing
    // selected, so the trigger offers the list rather than naming a changeset.
    expect(host.querySelector("[data-pane-summary]").textContent).toBe("Pick a commit");
    pane.dispose();
  });

  it("hands the Files browser's tree to the primitive", async () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const files = renderFilesTab(host, {
      scope: { run_id: "run-1" },
      callRpc: async () => ({ path: "", entries: [{ name: "src", kind: "dir" }, { name: "a.js", kind: "file", size: 3 }] }),
    });
    await vi.waitFor(() => expect(host.querySelector(".ffile")).toBeTruthy());
    const split = host.querySelector(".files");
    expect(split.querySelector("#ftree").classList.contains("pane-list")).toBe(true);
    split.querySelector("[data-pane-handle]").click();
    // A directory only moves the tree — you are still choosing.
    split.querySelector(".fdir").click();
    expect(isOpen(split)).toBe(true);
    await vi.waitFor(() => expect(host.querySelector(".ffile")).toBeTruthy());
    split.querySelector(".ffile").click();
    expect(isOpen(split)).toBe(false);
    files.dispose();
  });

  it("names the open file on the Files trigger, and offers the tree until there is one", async () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const files = renderFilesTab(host, {
      scope: { run_id: "run-1" },
      callRpc: async (method) =>
        method === "fs.tree"
          ? { path: "src", entries: [{ name: "a.js", kind: "file", size: 3 }] }
          : { mime: "text/plain", size: 3, content_b64: "YWJj", truncated: false },
      openAt: { path: "src/a.js", line: null },
    });
    const says = () => host.querySelector("[data-pane-summary]").textContent;
    expect(says()).toBe("Choose a file");
    await vi.waitFor(() => expect(says()).toBe("src/a.js"));
    files.dispose();
  });
});
