/** @vitest-environment jsdom */
// Visibility is exercised through real workspace Changes panes. Only the
// neighboring console, agent rail, and feed are stood in for.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { worktreeOf } from "./gitWireFixture.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

vi.mock("../src/core/console.js", () => ({ mountConsole: vi.fn(() => ({ dispose: vi.fn() })) }));
vi.mock("../src/core/agentRail.js", () => ({ mountAgentRail: vi.fn(() => ({ dispose: vi.fn() })) }));

let feedWorkspaces = [];
let feedSubscribers = new Set();
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => {
    feedSubscribers.add(fn);
    fn({ devices: { "dev-1": { items: [], projects: [], workspaces: feedWorkspaces } } });
    return () => feedSubscribers.delete(fn);
  },
  startFeed: () => {}, stopFeed: () => {}, refreshFeed: async () => [true],
  deliverFeed: () => {}, dropFeedDevice: () => {}, joinFeed: () => {},
}));

import { App } from "../src/app.js";
import { renderWorkspace } from "../src/views/workspaceView.js";
import { adoptDeviceSession, resetDeviceContexts } from "../src/core/deviceContexts.js";
import { standShell, stopShell } from "../src/core/shell.js";
import { fakeSession } from "./deviceSessionFixture.js";
import { readCached, wipeCache, writeCached } from "../src/core/localCache.js";
import { directoryCacheId } from "../src/core/directoryScope.js";
import { armChangeEvents, disarmChangeEvents, dispatchChangeEvent, refetchEverything } from "../src/core/changeEvents.js";
import { createViewingContext } from "../src/core/viewingContext.js";

const workspace = {
  id: "ws-1", project_id: "p-1", name: "payment-work",
  directories: [
    { source_id: "repo", name: "Repository", is_git: true },
    { source_id: "assets", name: "Assets", is_git: true },
  ],
};
let trees;
const asked = [];
const listing = {
  current: { kind: "branch", name: "main", full_ref: "refs/heads/main", current: true },
  refs: [
    { kind: "branch", name: "main", full_ref: "refs/heads/main", current: true },
    { kind: "branch", name: "feature", full_ref: "refs/heads/feature", current: false },
  ],
};
let rpcOverride;
const machine = async (method, params = {}) => {
  asked.push({ method, params });
  const override = rpcOverride?.(method, params);
  if (override !== undefined) return override;
  if (method === "git.status") return trees[params.source_id].status();
  if (method === "git.diff") return trees[params.source_id].diff(params);
  if (method === "git.log") return { branch: "main", commits: [], more: false };
  if (method === "git.refs") return listing;
  if (method === "git.unpushed") return { commits: [], files: [] };
  if (method === "workspace.git_init_options") return {
    workspace_id: "ws-1", source_id: params.source_id,
    workspace: { path: `/w/${params.source_id}`, is_git: true, available: true },
    source: { path: `/src/${params.source_id}`, is_git: true, available: true },
  };
  return {};
};
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const surface = (id) => document.querySelector(`[data-surface="${id}"]`);
const tab = (id) => document.querySelector(`[data-directory="${id}"]`);
const file = (id) => surface(id)?.querySelector(".file");
const open = async () => {
  App.route = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "repo", tab: "changes" };
  standShell(App.route);
  await renderWorkspace();
  await vi.waitFor(() => expect(surface("repo")?.querySelector('.rrow[data-sel="uncommitted"]')).toBeTruthy());
  surface("repo").querySelector('.rrow[data-sel="uncommitted"]').click();
  await vi.waitFor(() => expect(file("repo")?.querySelector(".fcmt")).toBeTruthy());
};
const switchTo = (id) => {
  tab(id).dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
  tab(id).click();
  expect(App.route.sourceId).toBe(id);
};

beforeEach(async () => {
  await wipeCache();
  document.body.innerHTML = '<div id="toolbar"><span id="tb-verb"></span></div><nav id="dir-rail"></nav><div id="root"></div><aside id="agent-rail"></aside><div id="console-region"></div>';
  App.viewDispose = null;
  App.viewingContext = createViewingContext({ enabled: false });
  App.devices = [{ id: "dev-1", name: "this machine", status: "online" }];
  asked.length = 0;
  rpcOverride = null;
  trees = {
    repo: worktreeOf({ "repo-only.js": "changed in the repository" }),
    assets: worktreeOf({ "asset-only.js": "changed in the assets" }),
  };
  feedSubscribers = new Set();
  feedWorkspaces = [workspace];
  adoptDeviceSession({ ...fakeSession("dev-1"), call: vi.fn(machine) });
  window.matchMedia = () => ({ matches: false, addEventListener() {}, removeEventListener() {} });
});
afterEach(() => {
  App.viewDispose?.();
  App.viewDispose = null;
  stopShell();
  resetDeviceContexts();
  disarmChangeEvents("dev-1");
  delete window.matchMedia;
  document.getSelection()?.removeAllRanges();
  vi.useRealTimers();
});

describe("a Changes directory that is mounted but hidden", () => {
  it("suspends an open reference menu, its keyboard focus, and Escape handling", async () => {
    await open();
    const repo = surface("repo");
    const trigger = repo.querySelector(".workspace-reftrigger");
    await vi.waitFor(() => expect(trigger.disabled).toBe(false));
    trigger.click();
    const menu = repo.querySelector(".workspace-refmenu");
    expect(menu.hidden).toBe(false);
    expect(document.activeElement).toBe(repo.querySelector(".workspace-refsearch"));

    switchTo("assets");
    expect(repo.hidden).toBe(true);
    expect(menu.hidden).toBe(true);
    expect(repo.contains(document.activeElement)).toBe(false);
    const shown = surface("assets");
    await vi.waitFor(() => expect(shown.querySelector(".workspace-reftrigger")?.disabled).toBe(false));
    const shownTrigger = shown.querySelector(".workspace-reftrigger");
    shownTrigger.focus();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(document.activeElement).toBe(shownTrigger);

    switchTo("repo");
    expect(menu.hidden).toBe(true);
    expect(trigger.getAttribute("aria-expanded")).toBe("false");
  });

  it("closes a file menu and disarms its destructive timer when leaving", async () => {
    await open();
    file("repo").querySelector(".fmenu").click();
    await vi.waitFor(() => expect(file("repo").querySelector(".gitdiscard")).not.toBeNull());
    file("repo").querySelector(".gitdiscard").click();
    await vi.waitFor(() => expect(file("repo").querySelector(".gitdiscard")?.textContent).toContain("Discard changes?"));
    switchTo("assets");
    switchTo("repo");
    expect(file("repo").querySelector(".gitdiscard")).toBeNull();
    expect(asked.some(({ method }) => method === "git.discard")).toBe(false);
  });

  it("keeps file marks and scroll, while clearing a browser text range in the hidden diff", async () => {
    await open();
    const repo = surface("repo");
    const checkbox = file("repo").querySelector(".fselect-box");
    checkbox.click();
    await vi.waitFor(() => expect(file("repo").querySelector(".fselect-box").checked).toBe(true));
    const scroller = repo.querySelector(".cdetail-host");
    scroller.scrollTop = 137;
    const walker = document.createTreeWalker(file("repo"), NodeFilter.SHOW_TEXT);
    let textNode = walker.nextNode();
    while (textNode && !textNode.textContent.trim()) textNode = walker.nextNode();
    expect(textNode).toBeTruthy();
    const range = document.createRange();
    range.selectNodeContents(textNode);
    document.getSelection().removeAllRanges();
    document.getSelection().addRange(range);
    expect(document.getSelection().isCollapsed).toBe(false);

    switchTo("assets");
    expect(document.getSelection().isCollapsed).toBe(true);
    switchTo("repo");
    expect(repo.querySelector(".cdetail-host").scrollTop).toBe(137);
    expect(file("repo").querySelector(".fselect-box").checked).toBe(true);
  });

  it("cancels a queued native text-selection comment when its directory hides", async () => {
    await open();
    await vi.waitFor(() => expect(file("repo")?.querySelector("tr[data-ln]")).toBeTruthy());
    const row = file("repo").querySelector("tr[data-ln]");
    const walker = document.createTreeWalker(row, NodeFilter.SHOW_TEXT);
    let textNode = walker.nextNode();
    while (textNode && !textNode.textContent.trim()) textNode = walker.nextNode();
    expect(textNode).toBeTruthy();
    const range = document.createRange();
    range.selectNodeContents(textNode);
    document.getSelection().removeAllRanges();
    document.getSelection().addRange(range);
    row.dispatchEvent(new Event("pointerup", { bubbles: true }));
    document.dispatchEvent(new Event("selectionchange"));
    switchTo("assets");
    await new Promise((resolve) => setTimeout(resolve, 400));
    expect(document.querySelector("body > .comment-pop")).toBeNull();
    expect(document.getSelection().isCollapsed).toBe(true);
  });

  it("pauses a hidden pane's relative-time timer and refreshes its label on return", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
    const editedAt = Date.now() - 30_000;
    rpcOverride = (method, params) => {
      if (method !== "git.status" || params.source_id !== "repo") return undefined;
      const status = trees.repo.status();
      return { ...status, files: status.files.map((entry) => ({ ...entry, edited_at: editedAt })) };
    };
    await open();
    const timestamp = file("repo").querySelector(".fedited");
    expect(timestamp?.textContent).toBe("Just Now");
    switchTo("assets");
    await vi.advanceTimersByTimeAsync(60_000);
    expect(timestamp.textContent).toBe("Just Now");
    switchTo("repo");
    expect(file("repo").querySelector(".fedited").textContent).toBe("1 minute ago");
  });

  it("keeps a suspended draft and its DOM through a hidden status cache push", async () => {
    await open();
    const repo = surface("repo");
    const before = file("repo");
    before.querySelector(".fcmt").click();
    document.querySelector(".cp-input").value = "Review in progress";
    switchTo("assets");
    expect(document.querySelector("body > .comment-pop")).toBeNull();
    trees.repo.write("new-file.js", "arrived while hidden");
    await writeCached({ deviceId: "dev-1", entityId: directoryCacheId({ workspace_id: "ws-1", source_id: "repo" }), kind: "status" }, trees.repo.status());
    await flush();
    expect(repo.querySelector(".file[data-key$='new-file.js']")).toBeNull();
    expect(file("repo")).toBe(before);
    switchTo("repo");
    expect(document.querySelector(".cp-input").value).toBe("Review in progress");
    document.querySelector(".cp-save").click();
    await vi.waitFor(() => expect(repo.querySelector(".file[data-key$='new-file.js']")).not.toBeNull());
  });

  it("defers a bridge board invalidation and refreshes the hidden checkout when it returns", async () => {
    await open();
    switchTo("assets");
    await vi.waitFor(() => expect(surface("assets")?.querySelector(".workspace-reftrigger")).toBeTruthy());
    const priorReads = asked.filter(({ method, params }) => method === "git.status" && params.source_id === "repo").length;
    trees.repo.write("pushed-file.js", "news from the bridge");
    armChangeEvents({ push_events: true }, "dev-1");
    expect(dispatchChangeEvent({ type: "changes", items: [{ entity_id: "board", state: {} }] }, "dev-1")).toBe(true);
    await flush();
    expect(asked.filter(({ method, params }) => method === "git.status" && params.source_id === "repo")).toHaveLength(priorReads);
    expect(surface("repo").querySelector(".file[data-key$='pushed-file.js']")).toBeNull();

    switchTo("repo");
    await vi.waitFor(() => expect(asked.filter(({ method, params }) => method === "git.status" && params.source_id === "repo")).toHaveLength(priorReads + 1));
    await vi.waitFor(() => expect(surface("repo").querySelector(".file[data-key$='pushed-file.js']")).toBeTruthy());
  });

  it.each([false, true])("refreshes cached Git records after checkout (old reads pending: %s)", async (pending) => {
    let branch = "main";
    let holdOld = false;
    const heldReads = new Map();
    const answer = (method, value) => {
      if (!holdOld || branch !== "main") return value;
      return new Promise((resolve) => heldReads.set(method, () => resolve(value)));
    };
    rpcOverride = (method, params) => {
      if (params.source_id !== "repo") return undefined;
      if (method === "git.checkout_ref") {
        branch = "feature";
        trees.repo.write("checkout-only.js", "checked out branch");
        return {};
      }
      if (method === "git.refs") return { ...listing, current: { kind: "branch", name: branch } };
      if (method === "git.status") return answer(method, trees.repo.status({ branch }));
      if (method === "git.log") return answer(method, { branch, commits: [{ hash: branch, short: branch, subject: `${branch} history` }], more: false });
      if (method === "git.unpushed") return answer(method, { patch: trees.repo.wholePatch(), diff_key: branch, base: { kind: "push_target", label: `origin/${branch}` } });
      return undefined;
    };
    await open();
    const repo = surface("repo");
    repo.querySelector('.rrow[data-sel="review"]').click();
    await vi.waitFor(async () => {
      for (const kind of ["refs", "status", "log", "unpushed", "diff"]) {
        expect((await readCached({ deviceId: "dev-1", entityId: directoryCacheId({ workspace_id: "ws-1", source_id: "repo" }), kind }))?.value).toBeTruthy();
      }
    });
    if (pending) {
      await vi.waitFor(() => expect(repo.textContent).toContain("changed in the repository"));
      holdOld = true;
      refetchEverything("dev-1");
      await vi.waitFor(() => expect([...heldReads.keys()].sort()).toEqual(["git.log", "git.status", "git.unpushed"]));
    }
    await vi.waitFor(() => expect(repo.querySelector(".workspace-reftrigger").disabled).toBe(false));
    repo.querySelector(".workspace-reftrigger").click();
    repo.querySelector('[data-ref="refs/heads/feature"]').click();
    await vi.waitFor(() => expect(repo.textContent).toContain("feature history"));
    await vi.waitFor(() => expect(repo.textContent).toContain("checked out branch"));
    await vi.waitFor(() => expect(repo.querySelector(".workspace-reftrigger-name").textContent).toBe("feature"));
    await vi.waitFor(() => expect(repo.querySelector('.rrow[data-sel="review"] .rsub').textContent).toBe("vs origin/feature"));
    for (const finish of heldReads.values()) finish();
    for (let turn = 0; turn < 20; turn += 1) await flush();
    const cached = async (kind) => (await readCached({ deviceId: "dev-1", entityId: directoryCacheId({ workspace_id: "ws-1", source_id: "repo" }), kind }))?.value;
    expect((await cached("status")).branch).toBe("feature");
    expect((await cached("log")).branch).toBe("feature");
    expect((await cached("unpushed")).base.label).toBe("origin/feature");
    expect((await cached("diff")).diff_key).toBe("feature");
  });

  it("keeps a pending checkout completion on its own directory", async () => {
    let completeCheckout;
    rpcOverride = (method, params) => {
      if (method === "git.checkout_ref" && params.source_id === "repo") return new Promise((resolve) => { completeCheckout = resolve; });
      return undefined;
    };
    await open();
    const repo = surface("repo");
    await vi.waitFor(() => expect(repo.querySelector(".workspace-reftrigger").disabled).toBe(false));
    repo.querySelector(".workspace-reftrigger").click();
    repo.querySelector('[data-ref="refs/heads/feature"]').click();
    expect(completeCheckout).toBeTypeOf("function");
    switchTo("assets");
    const assets = surface("assets");
    await vi.waitFor(() => expect(assets.querySelector('.rrow[data-sel="uncommitted"]')).toBeTruthy());
    assets.querySelector('.rrow[data-sel="uncommitted"]').click();
    await vi.waitFor(() => expect(file("assets")).toBeTruthy());
    file("assets").querySelector(".fcmt").click();
    const assetsEditor = document.querySelector("body > .comment-pop .cp-input");
    expect(assetsEditor).toBeTruthy();
    assetsEditor.value = "Assets owns this comment";
    completeCheckout({});
    await flush();
    expect(assets.hidden).toBe(false);
    expect(file("assets")).not.toBeNull();
    expect(App.route.sourceId).toBe("assets");
    expect(document.querySelector("body > .comment-pop .cp-input")).toBe(assetsEditor);
    expect(assetsEditor.value).toBe("Assets owns this comment");
    document.querySelector("body > .comment-pop .cp-save").click();
    await vi.waitFor(() => expect(assets.textContent).toContain("Assets owns this comment"));
    expect(repo.textContent).not.toContain("Assets owns this comment");
    switchTo("repo");
    expect(repo.hidden).toBe(false);
    await vi.waitFor(() => expect(repo.querySelector(".workspace-reftrigger")?.disabled).toBe(false));
  });
});
