// @vitest-environment jsdom
// The way out of a diff and into the file itself: a control in each file's
// head, a URL that names the file, and a Files view that opens on that path at
// that line. The file is in the URL, so the way out is also a link somebody can
// send.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { mountGitPane } from "../src/core/gitPane.js";
import { createReviewPlug } from "../src/core/changesReview.js";
import { renderFilesTab } from "../src/views/files.js";
import { worktreeOf } from "./gitWireFixture.js";

const patchFor = (path, line) =>
  `diff --git a/${path} b/${path}\nindex 1111111..2222222 100644\n--- a/${path}\n+++ b/${path}\n@@ -12,2 +12,2 @@\n-old\n+${line}\n`;

const tree = worktreeOf({ "src/a.js": "first" }, { patchOf: patchFor });
const status = (over = {}) => tree.status(over);

const log = () => ({ branch: "main", commits: [], more: false });

const settle = async () => {
  for (let i = 0; i < 8; i++) await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
};

const click = async (element) => {
  element.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
  await settle();
};

beforeEach(() => {
  document.body.innerHTML = "";
});

afterEach(() => {
  document.body.innerHTML = "";
  vi.useRealTimers();
});

describe("the control in a file's head", () => {
  it("sends the Changes pane's reader to that file, at the line the diff is about", async () => {
    const opened = [];
    const container = document.createElement("div");
    document.body.appendChild(container);
    const callRpc = vi.fn(async (method, params) => {
      if (method === "git.status") return status();
      if (method === "git.diff") return tree.diff(params);
      if (method === "git.log") return log();
      return {};
    });
    const pane = mountGitPane(container, {
      scope: { run_id: "run-1" },
      callRpc,
      navigate: { openFile: (where) => opened.push(where) },
    });
    await settle();
    const file = container.querySelector('.file[data-key$=":src/a.js"]');
    expect(file.classList.contains("capped")).toBe(true);
    await click(file.querySelector(".fopen"));
    expect(opened).toEqual([{ path: "src/a.js", line: 12 }]);
    // The press was the control's, not the fold's.
    expect(container.querySelector('.file[data-key$=":src/a.js"]').classList.contains("capped")).toBe(true);
    pane.dispose();
  });

  it("offers none where the surface has nowhere to send the reader", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const callRpc = vi.fn(async (method, params) => {
      if (method === "git.status") return status();
      if (method === "git.diff") return tree.diff(params);
      if (method === "git.log") return log();
      return {};
    });
    const pane = mountGitPane(container, { scope: { run_id: "run-1" }, callRpc });
    await settle();
    expect(container.querySelector(".fopen")).toBe(null);
    pane.dispose();
  });

  it("sends the review plug's reader the same way", async () => {
    vi.useFakeTimers();
    const opened = [];
    const host = document.createElement("div");
    document.body.appendChild(host);
    const plug = createReviewPlug({
      fetchDiff: async () => ({ patch: patchFor("src/a.js", "first") }),
      navigate: { openFile: (where) => opened.push(where) },
    });
    plug.mount(host);
    await vi.advanceTimersByTimeAsync(0);
    host.querySelector(".fopen").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(0);
    expect(opened).toEqual([{ path: "src/a.js", line: 12 }]);
    expect(host.querySelector(".file").classList.contains("capped")).toBe(true);
    plug.unmount();
  });
});

describe("the Files view, opened at a line", () => {
  const source = ["one", "two", "three", "four"].join("\n");

  const mountFiles = (openAt) => {
    const scrolled = [];
    const original = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = function (options) {
      scrolled.push({ line: this.dataset.newLine, options });
    };
    const host = document.createElement("div");
    document.body.appendChild(host);
    const files = renderFilesTab(host, {
      scope: { run_id: "run-1" },
      openAt,
      callRpc: async (method) => {
        if (method === "fs.tree") return { path: "src", entries: [{ kind: "file", name: "a.js", size: 20 }] };
        return {
          path: "src/a.js",
          size: 20,
          truncated: false,
          mime: "text/plain",
          content_b64: btoa(source),
        };
      },
    });
    return {
      host,
      files,
      scrolled,
      restore: () => {
        Element.prototype.scrollIntoView = original;
      },
    };
  };

  it("marks every source row with its line", async () => {
    const { host, files, restore } = mountFiles({ path: "src/a.js", line: null });
    await vi.waitFor(() => expect(host.querySelector(".fsrc")).toBeTruthy());
    expect([...host.querySelectorAll(".fsrc tr")].map((row) => row.dataset.newLine)).toEqual(["1", "2", "3", "4"]);
    files.dispose();
    restore();
  });

  it("scrolls the line it was sent to into the middle of the view", async () => {
    const { host, files, scrolled, restore } = mountFiles({ path: "src/a.js", line: 3 });
    await vi.waitFor(() => expect(host.querySelector(".fsrc")).toBeTruthy());
    expect(scrolled).toEqual([{ line: "3", options: { block: "center" } }]);
    files.dispose();
    restore();
  });

  it("opens a file named with no line without scrolling anywhere", async () => {
    const { host, files, scrolled, restore } = mountFiles({ path: "src/a.js" });
    await vi.waitFor(() => expect(host.querySelector(".fsrc")).toBeTruthy());
    expect(scrolled).toEqual([]);
    files.dispose();
    restore();
  });
});

describe("the line a jump asked for", () => {
  const mountTwoFiles = (openAt) => {
    const scrolled = [];
    const original = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = function (options) {
      scrolled.push({ line: this.dataset.newLine, options });
    };
    const host = document.createElement("div");
    document.body.appendChild(host);
    const files = renderFilesTab(host, {
      scope: { run_id: "run-1" },
      openAt,
      callRpc: async (method, params) => {
        if (method === "fs.tree")
          return {
            path: "src",
            entries: [
              { kind: "file", name: "shot.png", size: 20 },
              { kind: "file", name: "b.js", size: 20 },
            ],
          };
        if (params.path === "src/shot.png")
          return { path: params.path, size: 20, truncated: false, mime: "image/png", content_b64: btoa("x") };
        return {
          path: params.path,
          size: 20,
          truncated: false,
          mime: "text/plain",
          content_b64: btoa(["one", "two", "three", "four"].join("\n")),
        };
      },
    });
    return {
      host,
      files,
      scrolled,
      restore: () => {
        Element.prototype.scrollIntoView = original;
      },
    };
  };

  it("does not land on the next file the reader opens when its own had no lines to land on", async () => {
    const { host, files, scrolled, restore } = mountTwoFiles({ path: "src/shot.png", line: 3 });
    await vi.waitFor(() => expect(host.querySelector(".fimg")).toBeTruthy());
    expect(scrolled).toEqual([]);

    host.querySelectorAll(".ffile")[1].dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await vi.waitFor(() => expect(host.querySelector(".fsrc")).toBeTruthy());
    expect(scrolled).toEqual([]);
    files.dispose();
    restore();
  });
});

describe("the file the route names", () => {
  const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
  const flush = () => new Promise((done) => setTimeout(done, 0));
  const row = {
    kind: "branch",
    project_id: "p1",
    project: "relaydb",
    branch: "build/login",
    worktree_id: "wt-1",
    agents: [],
  };

  let App;
  let renderBranch;

  beforeEach(async () => {
    vi.resetModules();
    document.body.innerHTML = bodyHtml;
    location.hash = "#/p/p1/branch/build%2Flogin/files";
    ({ App } = await import("../src/app.js"));
    ({ renderBranch } = await import("../src/views/branchView.js"));
    document.getElementById("toolbar").innerHTML = '<span id="tb-verb"></span>';
  });

  afterEach(() => {
    if (App.poll) App.poll.dispose();
    App.poll = null;
    if (App.viewDispose) App.viewDispose();
    App.viewDispose = null;
  });

  const answering = (asked) =>
    vi.fn(async (method, params) => {
      if (method === "branch.get") return row;
      asked.push({ method, params });
      if (method === "fs.tree")
        return { path: params.path || "", entries: [{ kind: "file", name: params.path ? "a.js" : "README.md", size: 20 }] };
      if (method === "fs.read")
        return { path: params.path, size: 4, truncated: false, mime: "text/plain", content_b64: btoa("x\ny\n") };
      return {};
    });

  it("opens the Files tab on the file the URL names", async () => {
    const asked = [];
    App.call = answering(asked);
    App.route = { name: "branch", projectId: "p1", branch: "build/login", tab: "files", file: "src/a.js", line: 2 };
    await renderBranch();
    await flush();
    await vi.waitFor(() => expect(document.querySelector(".fppath")?.textContent).toBe("src/a.js"));
    expect(asked.find((call) => call.method === "fs.read").params.path).toBe("src/a.js");
  });

  // The reviewer's bug: branchView wrapped the review plug in a hand-written
  // subset — `mount(host)` swallowing the pane's options, and no `commentOffer`
  // at all — so the pane threw part-way through its render and everything after
  // that line went missing: Pull, Push, Stash, the merge verb, and the box under
  // the diff. The wrapper spreads the plug now, and this is what says so.
  it("renders the whole Changes surface, toolbar verbs and box included", async () => {
    const asked = [];
    App.call = vi.fn(async (method, params) => {
      if (method === "branch.get")
        return { ...row, run_id: "run-1", run: { run_id: "run-1", state: "review", base_branch: "main", thread: { items: [], sessions: [] } } };
      asked.push({ method, params });
      if (method === "git.status")
        return {
          branch: "build/login", head: "abc", repo_state: "clean", upstream: "origin/build/login",
          ahead: 0, behind: 0, files: [], stat: { files_changed: 0, insertions: 0, deletions: 0 },
        };
      if (method === "git.log") return { branch: "build/login", commits: [], more: false };
      if (method === "run.diff") return { patch: "" };
      return {};
    });
    App.route = { name: "branch", projectId: "p1", branch: "build/login", tab: "changes" };
    await renderBranch();
    await flush();
    await vi.waitFor(() => expect(document.querySelector(".gittoolbar")).toBeTruthy());
    await vi.waitFor(() => expect(document.querySelector(".gtpull .btn")).toBeTruthy());

    expect(document.querySelector(".gtpush .btn"), "Push").toBeTruthy();
    expect(document.querySelector(".gtstash .btn"), "Stash").toBeTruthy();
    expect(document.querySelector(".gp-commit .csinput"), "the box under the diff").toBeTruthy();
  });

  it("leaves an ordinary visit to the Files tab at the root", async () => {
    const asked = [];
    App.call = answering(asked);
    App.route = { name: "branch", projectId: "p1", branch: "build/login", tab: "files" };
    await renderBranch();
    await flush();
    await vi.waitFor(() => expect(document.querySelector(".ffile")).toBeTruthy());
    expect(asked.filter((call) => call.method === "fs.read")).toEqual([]);
    expect(asked.find((call) => call.method === "fs.tree").params.path).toBe("");
  });

  // The URL is what makes a file sendable, so picking one inside the tab has to
  // move it too — silently, without rebuilding the surface around the file it
  // is already showing.
  it("writes the file the reader picks into the URL, without a re-render", async () => {
    const asked = [];
    App.call = answering(asked);
    App.route = { name: "branch", projectId: "p1", branch: "build/login", tab: "files" };
    await renderBranch();
    await flush();
    await vi.waitFor(() => expect(document.querySelector(".ffile")).toBeTruthy());
    const built = document.querySelector(".files");

    document.querySelector(".ffile").dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
    await vi.waitFor(() => expect(App.route.file).toBe("README.md"));
    expect(location.hash).toContain("path=README.md");
    expect(document.querySelector(".files")).toBe(built); // the same surface, still standing
  });
});
