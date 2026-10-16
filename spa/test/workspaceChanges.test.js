// @vitest-environment jsdom
// The Changes tab row (#174): one tab per workspace directory, git or not, the
// current one selected and the row's one tab stop — and a tab moving within
// the surface, each directory's kept. The git pane is stood in for here, as a
// pane that says what the reader is looking at; test/workspaceNavWiring.test.js
// mounts the real one.

import { describe, expect, it, vi } from "vitest";

const panes = vi.hoisted(() => new Map());
/** A commit each stood-in pane has selected: the context takes a real sha. */
const SHA = vi.hoisted(() => ({ repo: "a1".repeat(20), site: "b2".repeat(20), later: "c3".repeat(20) }));
vi.mock("../src/core/gitPane.js", () => ({
  mountGitPane: vi.fn((host, { scope, viewingContext }) => {
    host.innerHTML = `<aside class="crail-host">${scope.source_id}</aside>`;
    const pane = {
      say: (sha) => viewingContext.set({ kind: "commit", sha }),
      snapshot: () => viewingContext.snapshot(),
      dispose: vi.fn(), setVisible: vi.fn(),
    };
    panes.set(scope.source_id, pane);
    pane.say(SHA[scope.source_id]);
    return pane;
  }),
}));
vi.mock("../src/core/workspaceRefPicker.js", () => ({ mountWorkspaceRefPicker: () => ({ dispose() {} }) }));

import { directoryHasGit, directoryTabsHtml, mountWorkspaceChanges } from "../src/views/workspaceChanges.js";
import { createViewingContext } from "../src/core/viewingContext.js";

const paint = (html) => {
  const host = document.createElement("div");
  host.innerHTML = html;
  return host;
};

describe("the Changes tab row", () => {
  it("is a tablist of every directory, the current one selected and reachable by Tab", () => {
    const host = paint(directoryTabsHtml([
      { sourceId: "repo", label: "Repository", current: false },
      { sourceId: "assets", label: "Assets", current: true },
    ]));
    const row = host.querySelector("[role=tablist]");
    expect(row.getAttribute("aria-label")).toBe("Workspace directories");
    expect([...row.querySelectorAll("[role=tab]")].map((tab) => [tab.dataset.directory, tab.textContent, tab.getAttribute("aria-selected"), tab.tabIndex]))
      .toEqual([["repo", "Repository", "false", -1], ["assets", "Assets", "true", 0]]);
  });

  it("escapes names and ids, which come from the user and the repos", () => {
    const hostile = '<img src=x onerror=alert(1)>" onfocus="x';
    const html = directoryTabsHtml([{ sourceId: hostile, label: hostile, current: true }]);
    expect(html).not.toContain("<img");
    const tab = paint(html).querySelector("[role=tab]");
    expect(tab.dataset.directory).toBe(hostile);
    expect(tab.textContent).toBe(hostile);
  });

  it("treats only a record saying so as a directory without git", () => {
    expect([directoryHasGit({ is_git: false }), directoryHasGit({ is_git: true }), directoryHasGit({})]).toEqual([false, true, true]);
  });
});

describe("selecting a directory", () => {
  const DIRECTORIES = [
    { sourceId: "repo", label: "Repository", is_git: true },
    { sourceId: "site", label: "Site", is_git: true },
    { sourceId: "assets", label: "Assets", is_git: false },
  ];
  const mount = () => {
    panes.clear();
    document.body.innerHTML = "<div id=host></div>";
    const viewingContext = createViewingContext();
    const onSelectDirectory = vi.fn();
    const changes = mountWorkspaceChanges(document.querySelector("#host"), {
      directories: DIRECTORIES,
      current: "repo",
      viewingContext,
      git: (sourceId, context) => ({ scope: { workspace_id: "ws-1", source_id: sourceId }, viewingContext: context }),
      onSelectDirectory,
    });
    return { changes, viewingContext, onSelectDirectory };
  };
  const surface = (sourceId) => document.querySelector(`[data-surface="${sourceId}"]`);
  const tab = (sourceId) => document.querySelector(`.workspace-dirtab[data-directory="${sourceId}"]`);
  const saying = (viewingContext) => viewingContext.snapshot()?.items.map((item) => item.sha);

  it("keeps the shown commit context quiet when visibility is repeated", () => {
    const { changes, viewingContext } = mount();
    const before = viewingContext.snapshot();
    const changed = vi.fn();
    viewingContext.subscribe(changed);

    changes.setVisible(true);
    changes.setVisible(true);

    expect(changed).not.toHaveBeenCalled();
    expect(viewingContext.snapshot()).toEqual(before);
    changes.dispose();
  });

  it("retains a dismissed commit and selected text during repeated shows", () => {
    const { changes, viewingContext } = mount();
    const selection = { kind: "selection", path: "a.js", text: "keep this passage", line_start: 7, line_end: 7 };
    viewingContext.setSelection([selection]);
    viewingContext.remove(0);
    const before = { version: 1, items: [selection] };
    expect(viewingContext.snapshot()).toEqual(before);

    changes.setVisible(true);
    changes.setVisible(true);

    expect(viewingContext.snapshot()).toEqual(before);
    changes.dispose();
  });

  it("retains a hidden pane's captured context when hiding is repeated", () => {
    const { changes, viewingContext } = mount();
    const selection = { kind: "selection", path: "a.js", text: "hidden pane's passage" };
    viewingContext.setSelection([selection]);
    const before = viewingContext.snapshot();
    changes.setVisible(false);
    viewingContext.set({ kind: "file", path: "another-face.js" });
    expect(panes.get("repo").snapshot()).toEqual(before);

    changes.setVisible(false);

    expect(panes.get("repo").snapshot()).toEqual(before);
    expect(viewingContext.snapshot()).toEqual({ version: 1, items: [{ kind: "file", path: "another-face.js" }] });
    changes.dispose();
  });

  it("keeps a newly selected source hidden when directories change behind another face", () => {
    const { changes, viewingContext } = mount();
    const repository = panes.get("repo");
    const repositoryContext = repository.snapshot();
    changes.setVisible(false);
    const fileContext = { version: 1, items: [{ kind: "file", path: "another-face.js" }] };
    viewingContext.set(fileContext);
    const changed = vi.fn();
    viewingContext.subscribe(changed);

    changes.workspaceMoved([{ sourceId: "later", label: "New repository", is_git: true }]);
    changes.setVisible(false);

    expect(changed).not.toHaveBeenCalled();
    expect(viewingContext.snapshot()).toEqual(fileContext);
    expect(repository.snapshot()).toEqual(repositoryContext);
    const later = panes.get("later");
    expect(later.setVisible).toHaveBeenLastCalledWith(false);
    const hiddenContext = later.snapshot();
    const nextFileContext = { version: 1, items: [{ kind: "file", path: "next-file.js" }] };
    viewingContext.set(nextFileContext);
    changes.setVisible(false);
    later.say(SHA.site);

    expect(later.snapshot()).toEqual(hiddenContext);
    expect(viewingContext.snapshot()).toEqual(nextFileContext);
    changes.setVisible(true);
    expect(saying(viewingContext)).toEqual([SHA.site]);
    changes.dispose();
  });

  it("suspends the kept source panes when the whole Changes face is hidden", () => {
    const { changes, viewingContext } = mount();
    changes.setVisible(false);
    expect(panes.get("repo").setVisible).toHaveBeenLastCalledWith(false);
    panes.get("repo").say(SHA.later);
    changes.setVisible(true);
    expect(panes.get("repo").setVisible).toHaveBeenLastCalledWith(true);
    expect(saying(viewingContext)).toEqual([SHA.later]);
    expect(panes.get("repo").dispose).not.toHaveBeenCalled();
    changes.dispose();
  });

  it("keeps each directory's surface, and shows it again rather than mounting it again", () => {
    const { changes, onSelectDirectory } = mount();
    const repo = surface("repo");
    tab("assets").click();
    expect(onSelectDirectory).toHaveBeenLastCalledWith("assets");
    expect(surface("assets").querySelector(".workspace-gitinit")).not.toBeNull();
    expect(repo.hidden).toBe(true);
    tab("repo").click();
    expect(surface("repo")).toBe(repo);
    expect([repo.hidden, surface("assets").hidden]).toEqual([false, true]);
    expect(panes.get("repo").dispose).not.toHaveBeenCalled();
    changes.dispose();
    expect(panes.get("repo").dispose).toHaveBeenCalled();
  });

  it("walks the row from the keyboard, staying on it", () => {
    const { onSelectDirectory } = mount();
    tab("repo").focus();
    tab("repo").dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    expect(onSelectDirectory).toHaveBeenLastCalledWith("site");
    expect(document.activeElement).toBe(tab("site"));
    expect(tab("site").getAttribute("aria-selected")).toBe("true");
    document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true }));
    expect(document.activeElement).toBe(tab("assets"));
  });

  it("lets only the directory showing say what the reader is looking at", () => {
    const { viewingContext } = mount();
    expect(saying(viewingContext)).toEqual([SHA.repo]);
    tab("site").click();
    expect(saying(viewingContext)).toEqual([SHA.site]);
    // A hidden surface repainting keeps its word until it is shown again.
    panes.get("repo").say(SHA.later);
    expect(saying(viewingContext)).toEqual([SHA.site]);
    tab("assets").click();
    expect(saying(viewingContext)).toBeUndefined();
    tab("repo").click();
    expect(saying(viewingContext)).toEqual([SHA.later]);
  });
});
