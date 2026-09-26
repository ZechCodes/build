// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import { activateTab, closeTab, fileTabsHtml, openTab, readTabLayout, tabLabels } from "../src/core/fileTabsModel.js";

const layout = (tabs, active) => ({ tabs, active });

describe("the open-file tab set", () => {
  it("opens a file as a new active tab at the end", () => {
    expect(openTab(layout(["a.js"], "a.js"), "b.js")).toEqual(layout(["a.js", "b.js"], "b.js"));
  });

  it("activates an already-open file instead of opening it twice", () => {
    expect(openTab(layout(["a.js", "b.js"], "b.js"), "a.js")).toEqual(layout(["a.js", "b.js"], "a.js"));
  });

  it("switches to a tab", () => {
    expect(activateTab(layout(["a.js", "b.js"], "b.js"), "a.js")).toEqual(layout(["a.js", "b.js"], "a.js"));
    expect(activateTab(layout(["a.js"], "a.js"), "gone.js")).toEqual(layout(["a.js"], "a.js"));
  });

  it("closing the active tab shows the neighbour to its right, else to its left, else nothing", () => {
    expect(closeTab(layout(["a", "b", "c"], "b"), "b")).toEqual(layout(["a", "c"], "c"));
    expect(closeTab(layout(["a", "b", "c"], "c"), "c")).toEqual(layout(["a", "b"], "b"));
    expect(closeTab(layout(["a"], "a"), "a")).toEqual(layout([], null));
  });

  it("closing a background tab keeps the active one", () => {
    expect(closeTab(layout(["a", "b", "c"], "c"), "a")).toEqual(layout(["b", "c"], "c"));
  });
});

describe("readTabLayout", () => {
  it("reads a remembered layout, dropping what is not a path and an active tab that is not open", () => {
    expect(readTabLayout({ tabs: ["a", 4, "a", "", "b"], active: "zzz" })).toEqual(layout(["a", "b"], "a"));
    expect(readTabLayout({ tabs: ["a", "b"], active: "b" })).toEqual(layout(["a", "b"], "b"));
  });

  it("reads nothing as no tabs", () => {
    expect(readTabLayout(undefined)).toEqual(layout([], null));
    expect(readTabLayout({ tabs: "nope" })).toEqual(layout([], null));
  });
});

describe("fileTabsHtml", () => {
  const paint = (html) => {
    const host = document.createElement("div");
    host.innerHTML = html;
    return host;
  };

  it("draws one tab per open file, named by its file name and titled by its path", () => {
    const host = paint(fileTabsHtml(layout(["src/a.js", "README.md"], "README.md"), new Set()));
    const tabs = host.querySelectorAll('[role="tab"]');
    expect([...tabs].map((tab) => tab.textContent.trim())).toEqual(["a.js", "README.md"]);
    expect(tabs[0].getAttribute("title")).toBe("src/a.js");
    expect(tabs[1].getAttribute("aria-selected")).toBe("true");
    expect(tabs[0].getAttribute("aria-selected")).toBe("false");
    expect(host.querySelectorAll(".ftab-close")).toHaveLength(2);
    expect(host.querySelector(".ftab-close").getAttribute("aria-label")).toBe("Close a.js");
  });

  it("marks a tab with unsaved edits", () => {
    const host = paint(fileTabsHtml(layout(["a.js", "b.js"], "a.js"), new Set(["b.js"])));
    const [a, b] = host.querySelectorAll(".ftab");
    expect(a.classList.contains("dirty")).toBe(false);
    expect(b.classList.contains("dirty")).toBe(true);
    expect(b.querySelector(".ftab-close").getAttribute("aria-label")).toBe("Close b.js (unsaved)");
  });

  it("draws nothing with no tabs open", () => {
    expect(fileTabsHtml(layout([], null), new Set())).toBe("");
  });

  it("escapes hostile names and paths", () => {
    const hostile = '<img src=x onerror=alert(1)>/" onfocus="alert(2)';
    const html = fileTabsHtml(layout([hostile], hostile), new Set());
    expect(html).not.toContain("<img");
    expect(html).not.toContain('onfocus="alert(2)"');
    const host = paint(html);
    expect(host.querySelector("[data-tab-path]").dataset.tabPath).toBe(hostile);
  });
});

// #174: open-file tabs work across a workspace's roots. A tab is named by its
// file; when two open files share a name, each says its root before it.
describe("tabs across roots", () => {
  const roots = { repo: "Repository", assets: "Assets" };
  const key = (root, path) => JSON.stringify([root, path]);
  const locate = (tab) => {
    const [root, path] = JSON.parse(tab);
    return { root: { id: root, label: roots[root] }, path };
  };

  it("names a tab by its file, and by `root / name` when another open file shares it", () => {
    const tabs = [key("repo", "src/index.js"), key("assets", "index.js"), key("repo", "README.md")];
    const labels = tabLabels(tabs, locate);
    expect(tabs.map((tab) => labels.get(tab).name)).toEqual(["Repository / index.js", "Assets / index.js", "README.md"]);
    expect(labels.get(tabs[0]).title).toBe("Repository / src/index.js");
  });

  it("draws those names and keeps the tab's own key", () => {
    const tabs = [key("repo", "index.js"), key("assets", "index.js")];
    const host = document.createElement("div");
    host.innerHTML = fileTabsHtml(layout(tabs, tabs[0]), new Set(), tabLabels(tabs, locate));
    expect([...host.querySelectorAll(".ftab-name")].map((tab) => [tab.textContent, tab.dataset.tabPath])).toEqual([
      ["Repository / index.js", tabs[0]],
      ["Assets / index.js", tabs[1]],
    ]);
  });

  it("reads a remembered layout keeping only the tabs the reader can still open", () => {
    const kept = key("repo", "a.js");
    const gone = key("elsewhere", "b.js");
    expect(readTabLayout({ tabs: [kept, gone], active: gone }, (tab) => tab !== gone)).toEqual(layout([kept], kept));
  });

  it("names a single checkout's tabs by path, as it always has", () => {
    const labels = tabLabels(["src/a.js", "lib/a.js"], (tab) => ({ root: { id: null, label: "" }, path: tab }));
    expect(labels.get("src/a.js")).toEqual({ name: "a.js", title: "src/a.js" });
  });
});
