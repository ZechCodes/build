// @vitest-environment jsdom
// The branch surface's lifecycle: a navigation that lands while the first read
// is still in flight must not leave a poll running for a view that is gone —
// the leaked-poller failure mode (every navigation orphans an interval that
// calls branch.get forever, and the app slows with each one).

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

/** The one bridge this file's device answers through: a test that hands over
 *  a new `call` is that bridge answering differently, not another machine. */
const bridge = { call: null };

// The surfaces below write what they read through the local cache, which is
// keyed by device: give the module a fake IndexedDB to do it in.
globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

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

/** A branch.get row with something to finish. `can_finish` is structural — is
 *  there anything here at all — and `finish.warnings` is what deleting it would
 *  cost, which the confirmation carries and never refuses over. */
const finishableRow = (over = {}) => ({
  ...row,
  state: "review",
  can_finish: true,
  finish: { warnings: [] },
  primary: false,
  issue_id: null,
  stat: { uncommitted: { files_changed: 0 }, ahead: 0, upstream: "origin/build/login" },
  ...over,
});

let App;
let renderBranch;
let shouldRetainDirtyFilesPane;
let resetDeviceContexts;

beforeEach(async () => {
  vi.resetModules();
  document.body.innerHTML = bodyHtml;
  location.hash = "#/p/p1/branch/build%2Flogin/changes";
  ({ App } = await import("../src/app.js"));
  ({ renderBranch, shouldRetainDirtyFilesPane } = await import("../src/views/branchView.js"));
  // The feed polls device contexts, so this file's one device has one: its call
  // is whatever the case in hand scripted onto bridge.call. It is the home device
  // too — this surface's branch is on the machine creation goes to — which is
  // what Done on it asks. Home is named the way the running app names it: the
  // account lists the device online and the pick names it. (The aliases are
  // left alone; pointing bridge.call at the delegate below would have it call
  // itself.)
  let adoptBridgeSelection, adoptDeviceSession;
  ({ adoptBridgeSelection, adoptDeviceSession, resetDeviceContexts } = await import(
    "../src/core/deviceContexts.js"
  ));
  App.devices = [{ id: "dev-1", name: "This device", status: "online" }];
  App.selectedDeviceId = "dev-1";
  const context = adoptDeviceSession({
    deviceId: "dev-1",
    call: (...args) => bridge.call(...args),
    close: () => {},
    peer: () => {},
    onCarrier: () => {},
  });
  // Its bridge has greeted, the way connection.js leaves every machine it
  // lands: the feed reads no device before that greeting has settled.
  adoptBridgeSelection(context, { major: 1, version: "1.0.0" }, {});
  App.route = { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login", tab: "changes" };
  // core/toolbar.js isn't mounted in this file — Done paints into its verb
  // slot (setToolbarVerb), so stand in for the one thing branchView.js needs
  // there: the slot existing, the way it always does once the app has booted.
  document.getElementById("toolbar").innerHTML = '<span id="tb-verb"></span>';
});

afterEach(() => {
  resetDeviceContexts();
  if (App.poll) clearInterval(App.poll);
  App.poll = null;
  if (App.viewDispose) App.viewDispose();
  App.viewDispose = null;
});

describe("the branch surface", () => {
  it("defers a background Files remount while its editor is dirty", () => {
    expect(shouldRetainDirtyFilesPane("files", { hasUnsavedChanges: () => true })).toBe(true);
    expect(shouldRetainDirtyFilesPane("files", { hasUnsavedChanges: () => false })).toBe(false);
    expect(shouldRetainDirtyFilesPane("changes", { hasUnsavedChanges: () => true })).toBe(false);
  });

  it("browses a plain folder without calling branch or git RPCs", async () => {
    const { stopFeed } = await import("../src/core/taskFeed.js");
    App.route = { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "main", tab: "files" };
    bridge.call = vi.fn(async (method) => {
      if (method === "board.list") return { items: [] };
      if (method === "project.list") return { projects: [{ project_id: "p1", name: "notes", is_git: false, base_branch: "main" }] };
      if (method === "fs.tree") return { path: "", entries: [{ name: "notes.txt", kind: "file", size: 12 }] };
      if (method === "fs.read") return { mime: "text/plain", size: 12, editable: true, encoding: "utf-8", revision: "notes-1", content_b64: btoa("folder notes") };
      throw new Error(`unexpected ${method}`);
    });
    await renderBranch();
    await vi.waitFor(() => expect(document.querySelector("#tabbody .ffile")).toBeTruthy());
    expect(document.querySelector("#tabbody .files")).toBeTruthy();
    expect(document.querySelector("#tabbody .ffile").textContent).toContain("notes.txt");
    document.querySelector("#tabbody .ffile").click();
    await vi.waitFor(() => expect(document.querySelector("#tabbody .fpbody")?.textContent).toContain("folder notes"));
    expect(bridge.call.mock.calls.some(([method]) => method === "branch.get" || method.startsWith("git."))).toBe(false);
    expect(document.querySelector('[data-tab="files"]').classList.contains("active")).toBe(true);
    stopFeed();
  });

  it("offers Git initialization in Changes and remounts Git after it succeeds", async () => {
    const { refreshFeed, stopFeed } = await import("../src/core/taskFeed.js");
    let initialized = false;
    App.route = { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "main", tab: "changes" };
    bridge.call = vi.fn(async (method) => {
      if (method === "board.list") return { items: initialized ? [{ ...row, branch: "main", primary: true, worktree_id: null }] : [] };
      if (method === "project.list") return { projects: [{ project_id: "p1", name: "notes", is_git: initialized, base_branch: "main" }] };
      if (method === "project.init_git") {
        initialized = true;
        return { project_id: "p1", name: "notes", is_git: true, base_branch: "main" };
      }
      if (method === "branch.get") return { ...row, branch: "main", primary: true, worktree_id: null };
      if (method === "git.status") return { files: [], head: "abc", status_key: "clean" };
      if (method === "git.log") return { commits: [] };
      return {};
    });
    await refreshFeed();
    await renderBranch();
    await flush();
    expect(document.querySelector("#init-git")).toBeTruthy();
    expect(bridge.call.mock.calls.some(([method]) => method.startsWith("git."))).toBe(false);

    document.querySelector("#init-git").click();
    await flush();
    await flush();
    expect(bridge.call).toHaveBeenCalledWith("project.init_git", { project_id: "p1" });
    expect(document.querySelector("#tabbody .gitpane")).toBeTruthy();
    stopFeed();
  });

  it("shows initialization failures and leaves a retry enabled", async () => {
    const { refreshFeed, stopFeed } = await import("../src/core/taskFeed.js");
    App.route = { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "main", tab: "changes" };
    bridge.call = vi.fn(async (method) => {
      if (method === "board.list") return { items: [] };
      if (method === "project.list") return { projects: [{ project_id: "p1", name: "notes", is_git: false }] };
      if (method === "project.init_git") throw new Error("disk is read-only");
      return {};
    });
    await refreshFeed();
    await renderBranch();
    document.querySelector("#init-git").click();
    await flush();
    const status = document.querySelector('[role="status"]');
    expect(status.textContent).toContain("disk is read-only");
    expect(document.querySelector("#init-git").disabled).toBe(false);
    stopFeed();
  });

  it("does not mount after navigation while project metadata is loading", async () => {
    let answerProjects;
    bridge.call = vi.fn((method) => {
      if (method === "project.list") return new Promise((resolve) => { answerProjects = resolve; });
      return Promise.resolve({});
    });
    const mounting = renderBranch();
    await flush();
    App.viewDispose();
    document.querySelector("#tabbody").innerHTML = '<div id="next-view">next</div>';
    answerProjects({ projects: [{ project_id: "p1", is_git: false }] });
    await mounting;
    expect(document.querySelector("#next-view")).toBeTruthy();
  });

  it("detects Git initialized by another client and replaces the folder prompt", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { refreshFeed, stopFeed } = await import("../src/core/taskFeed.js");
    let initialized = false;
    App.route = { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "main", tab: "changes" };
    bridge.call = vi.fn(async (method) => {
      if (method === "board.list") return { items: [] };
      if (method === "project.list") return { projects: [{ project_id: "p1", name: "notes", is_git: initialized }] };
      if (method === "branch.get") return { ...row, branch: "main", primary: true, worktree_id: null };
      if (method === "git.status") return { files: [], head: "abc" };
      if (method === "git.log") return { commits: [] };
      return {};
    });
    await refreshFeed();
    await renderBranch();
    expect(document.querySelector("#init-git")).toBeTruthy();
    initialized = true;
    await vi.advanceTimersByTimeAsync(2000);
    expect(document.querySelector("#tabbody .gitpane")).toBeTruthy();
    stopFeed();
    vi.useRealTimers();
  });

  // The reviewer's complaint: switching branches showed a bare loading frame
  // for the length of a round trip. The feed row stands the surface up first.
  it("stands the surface up from the feed row before the first read answers", async () => {
    const { refreshFeed, stopFeed } = await import("../src/core/taskFeed.js");
    bridge.call = vi.fn(async (method) => {
      if (method === "board.list") return { items: [finishableRow({ run_id: "run-1" })] };
      if (method === "project.list") return { projects: [] };
      if (method === "branch.get") return new Promise(() => {});
      return {};
    });
    await refreshFeed();
    renderBranch(); // never resolves here — the first read is still in flight
    await flush();
    expect(document.querySelector("#tabbody .gitpane")).toBeTruthy();
    stopFeed();
  });

  it("polls the row once mounted", async () => {
    bridge.call = vi.fn(async () => row);
    await renderBranch();
    await flush();
    expect(App.poll).not.toBeNull();
    expect(bridge.call).toHaveBeenCalledWith("branch.get", expect.objectContaining({ project_id: "p1" }));
  });

  // A bridge can be updated past this tab while the surface stands over it: the
  // session drops, re-greets, and settles unsupported. The mount already
  // happened, so nothing asks canAnswer again — the poll just calls the caller
  // it captured. It must be refused there, or a 1.x-shaped read goes at a 2.x
  // bridge and whatever comes back is painted under the update strip.
  it("stops polling a machine whose greeting settles unsupported under it", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { adoptBridgeSelection, contextFor } = await import("../src/core/deviceContexts.js");
    bridge.call = vi.fn(async () => row);
    await renderBranch();
    await flush();
    expect(bridge.call.mock.calls.some(([method]) => method === "branch.get")).toBe(true);

    adoptBridgeSelection(contextFor("dev-1"), { version: "9.0.0", unsupported: "bridge" }, null);
    bridge.call.mockClear();
    await vi.advanceTimersByTimeAsync(5000);

    expect(bridge.call).not.toHaveBeenCalled();
    expect(document.querySelector("#view .device-strip")?.textContent).toContain("This device");
    vi.useRealTimers();
  });

  // This poll runs every 1.6s and again on every change event, and the surface
  // renders no conversation — the rail beside it does, off its own paged read
  // of the same RPC. A read that names no bound is answered with every item the
  // conversation ever held, so a branch with hundreds of them shipped them all,
  // twice a second, to be thrown away unread.
  it("names a bound on the conversation it does not render", async () => {
    bridge.call = vi.fn(async () => row);
    await renderBranch();
    await flush();
    const reads = bridge.call.mock.calls.filter(([method]) => method === "branch.get").map(([, params]) => params);
    expect(reads.length).toBeGreaterThan(0);
    // The rail's read of the same RPC pages the conversation it paints; the
    // surface's own asks for the smallest page there is. Neither may go
    // unbounded — silence is what tells the daemon to ship every item.
    for (const params of reads)
      expect(params.thread_limit !== undefined || params.thread_after_sequence !== undefined).toBe(true);
    expect(reads.some((params) => params.thread_limit === 1)).toBe(true);
  });

  // Both tabs paint a .pane-split, which states the shell's gutters itself. In
  // a padded tab body those gutters are paid twice — a doubled inset all round —
  // and the body scrolls the two columns together instead of letting each scroll
  // in its own frame.
  it.each(["changes", "files"])("hands the %s pane a flush tab body", async (tab) => {
    App.route = { ...App.route, tab };
    bridge.call = vi.fn(async () => row);
    await renderBranch();
    await flush();
    expect(document.querySelector("#tabbody").classList.contains("flush")).toBe(true);
    if (tab === "files") expect(App.routeLeaveGuard).toEqual(expect.any(Function));
  });

  // The toolbar's create form arms this right before navigating here — the
  // one-shot signal that this landing is a branch just cut, nobody in it yet.
  it("focuses the rail's composer when the toolbar just cut this branch, and clears the flag", async () => {
    bridge.call = vi.fn(async () => row);
    App.focusComposerOnMount = true;
    await renderBranch();
    await flush();
    expect(document.getElementById("railinput")).toBe(document.activeElement);
    expect(App.focusComposerOnMount).toBe(false);
  });

  it("leaves focus alone on an ordinary visit", async () => {
    bridge.call = vi.fn(async () => row);
    await renderBranch();
    await flush();
    expect(document.getElementById("railinput")).not.toBe(document.activeElement);
  });

  it("installs no poll when the view was torn down mid-load", async () => {
    let answer;
    const firstRead = new Promise((r) => {
      answer = r;
    });
    bridge.call = vi.fn(() => firstRead);
    const mounting = renderBranch();
    // The user navigates away while branch.get is still in flight: the shell's
    // render() runs the outgoing view's teardown and clears its slots.
    App.viewDispose();
    App.viewDispose = null;
    App.poll = null;
    answer(row);
    await mounting;
    await flush();
    // A disposed view must not claim the poll slot the next view now owns.
    expect(App.poll).toBeNull();
  });

  // The branch never resolved: every tick fails the same way, and the empty
  // state is all there is. Repainting it would rebuild the one control on it.
  it("states a branch it cannot find once, and leaves the way out standing", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    bridge.call = vi.fn(async () => {
      throw new Error("unknown branch");
    });
    await renderBranch();
    await vi.advanceTimersByTimeAsync(0);
    const back = document.querySelector("#branchback");
    expect(back).toBeTruthy();

    await vi.advanceTimersByTimeAsync(4000); // two more failing reads

    expect(document.querySelector("#branchback"), "the empty state was rebuilt").toBe(back);
    vi.useRealTimers();
  });
});

// Every machine mints a `p1`, and a link names one of them. The surface a route
// opens is about the machine the route names — not about the machine creation
// goes to, and not about whichever machine's `p1` the merge happens to list
// first.
describe("a branch on another device", () => {
  let theirCall;
  let theirRow;

  beforeEach(async () => {
    const { adoptDeviceSession } = await import("../src/core/deviceContexts.js");
    App.devices = [
      { id: "dev-1", name: "This device", status: "online" },
      { id: "dev-2", name: "Desktop", status: "online" },
    ];
    App.selectedDeviceId = "dev-1"; // home stays this machine
    theirRow = { ...row, branch: "main", primary: true, worktree_id: null };
    theirCall = vi.fn(async (method) => {
      if (method === "board.list") return { items: [theirRow] };
      if (method === "project.list") return { projects: [{ project_id: "p1", name: "their notes", is_git: true }] };
      if (method === "branch.get") return theirRow;
      if (method === "git.status") return { files: [], head: "abc", status_key: "clean" };
      if (method === "git.log") return { commits: [] };
      if (method === "fs.tree") return { path: "", entries: [{ name: "notes.txt", kind: "file", size: 12 }] };
      if (method === "fs.read")
        return { mime: "text/plain", size: 12, editable: true, encoding: "utf-8", revision: "n-1", content_b64: btoa("their notes") };
      return {};
    });
    const { adoptBridgeSelection } = await import("../src/core/deviceContexts.js");
    adoptBridgeSelection(
      adoptDeviceSession({ deviceId: "dev-2", call: theirCall, close: () => {}, peer: () => {}, onCarrier: () => {} }),
      { major: 1, version: "1.0.0" },
      {},
    );
    App.route = { name: "branch", deviceId: "dev-2", projectId: "p1", branch: "main", tab: "changes" };
    // This machine holds a `p1` of its own, and it is a plain folder: anything
    // reading it instead of the desktop's says so on the screen.
    bridge.call = vi.fn(async (method) => {
      if (method === "board.list") return { items: [] };
      if (method === "project.list") return { projects: [{ project_id: "p1", name: "my notes", is_git: false }] };
      return {};
    });
  });

  const reached = (call, method) => call.mock.calls.some(([name]) => name === method);
  const reachedGit = (call) => call.mock.calls.some(([name]) => name.startsWith("git."));

  it("the view calls the route device's call, not the home device's", async () => {
    await renderBranch();
    await flush();

    expect(reached(theirCall, "branch.get")).toBe(true);
    expect(reachedGit(theirCall)).toBe(true);
    expect(reached(bridge.call, "branch.get")).toBe(false);
    expect(reachedGit(bridge.call)).toBe(false);
  });

  it("stands the surface up from the route device's row, not the home device's", async () => {
    const { refreshFeed, stopFeed } = await import("../src/core/taskFeed.js");
    theirCall.mockImplementation(async (method) => {
      if (method === "board.list") return { items: [theirRow] };
      if (method === "project.list") return { projects: [{ project_id: "p1", name: "their notes", is_git: true }] };
      if (method === "branch.get") return new Promise(() => {}); // the first read is still in flight
      if (method === "git.status") return { files: [], head: "abc", status_key: "clean" };
      if (method === "git.log") return { commits: [] };
      return {};
    });
    await refreshFeed();

    renderBranch();
    await flush();

    expect(document.querySelector("#tabbody .gitpane")).toBeTruthy();
    expect(document.querySelector("#init-git")).toBeNull();
    stopFeed();
  });

  it("Done finishes on the route's device", async () => {
    theirRow = finishableRow({ branch: "main", run_id: "run-7" });
    await renderBranch();
    await flush();
    document.querySelector("#tb-verb .btn.mini:not(.caret)").click();
    await flush();
    document.getElementById("confirm-scrim").querySelector("[data-confirm-ok]").click();
    await flush();

    expect(theirCall.mock.calls.find(([method]) => method === "branch.finish")[1]).toEqual({
      project_id: "p1",
      branch: "main",
      action: "delete",
    });
    expect(reached(bridge.call, "branch.finish")).toBe(false);
  });

  it("the Files tab marks a route that keeps the device", async () => {
    App.route = { ...App.route, tab: "files" };
    await renderBranch();
    await vi.waitFor(() => expect(document.querySelector("#tabbody .ffile")).toBeTruthy());

    document.querySelector("#tabbody .ffile").click();
    await flush();

    expect(App.route.deviceId).toBe("dev-2");
    expect(location.hash).toContain("#/device/dev-2/project/p1/branch/main/files");
    expect(location.hash).toContain("path=notes.txt");
  });

  it("a branch view for device B never reads or writes the cache under device A's key", async () => {
    const { readCached, writeCached, wipeCache } = await import("../src/core/localCache.js");
    await wipeCache();
    theirRow = { ...row, branch: "main", worktree_id: "wt-9" };
    // This machine holds a checkout of the same id, synced earlier.
    await writeCached({ deviceId: "dev-1", entityId: "wt-9", kind: "status" }, { files: [], head: "mine", status_key: "mine" });

    await renderBranch();
    await vi.waitFor(async () =>
      expect((await readCached({ deviceId: "dev-2", entityId: "wt-9", kind: "status" }))?.value.head).toBe("abc"),
    );

    expect((await readCached({ deviceId: "dev-1", entityId: "wt-9", kind: "status" })).value.head).toBe("mine");
  });

  // A machine that goes while its surface is open is a different case from a
  // link that arrives at one: the reader is already standing on what it read,
  // and that stays. All that is missing is whose state it is.
  it("keeps what was read when that machine goes, and names the machine over it", async () => {
    const { setContextOffline } = await import("../src/core/deviceContexts.js");
    await renderBranch();
    await flush();

    setContextOffline("dev-2");

    expect(document.querySelector("#root > .device-strip").textContent).toContain("Desktop isn't connected");
    expect(document.getElementById("tabbody")).toBeTruthy();
  });

  // The frozen sentence is a promise about what is on screen: this is what that
  // machine last said. A machine that goes before the first read lands has said
  // nothing here — the reader is looking at "loading…" over a frame that never
  // filled — so the surface says the plain thing instead.
  it("says the machine cannot be opened while nothing has been painted yet", async () => {
    const { setContextOffline } = await import("../src/core/deviceContexts.js");
    theirCall.mockImplementation(async (method) => (method === "branch.get" ? new Promise(() => {}) : {}));
    renderBranch();
    await flush();
    expect(document.getElementById("tabbody").textContent).toContain("loading…");

    setContextOffline("dev-2");

    expect(document.querySelector("#root > .device-strip").textContent).toBe(
      "Desktop isn't connected, so this can't be opened right now.",
    );
  });

  it("keeps the device on the tab bar's own links", async () => {
    await renderBranch();
    await flush();

    document.querySelector('[data-tab="files"]').click();
    await flush();

    expect(location.hash).toBe("#/device/dev-2/project/p1/branch/main/files");
  });

  // The machine answered once and has since gone. Its context is still here —
  // the drafts and cached reads on it survive the outage — but nothing can be
  // read through it, so the surface says which machine is missing instead of
  // standing a frame up over calls that will only be refused.
  it("an offline route device renders the offline state naming that device", async () => {
    const { setContextOffline } = await import("../src/core/deviceContexts.js");
    setContextOffline("dev-2");

    await renderBranch();
    await flush();

    expect(document.getElementById("root").textContent).toContain("Desktop isn't connected");
    expect(document.getElementById("tabbody")).toBeNull();
    expect(theirCall).not.toHaveBeenCalled();
    expect(bridge.call).not.toHaveBeenCalled();
  });
});

// A link can name a machine this client has no session with — a phone that has
// been shut, a link opened on a fresh browser. There is nothing to read and
// nothing to write until that machine answers, so the surface says so by name
// rather than painting a frame over an empty checkout.
describe("a branch on a device this client has not opened", () => {
  it("names the device and asks it nothing", async () => {
    App.devices = [...App.devices, { id: "dev-3", name: "Desktop", status: "offline" }];
    App.route = { name: "branch", deviceId: "dev-3", projectId: "p1", branch: "main", tab: "changes" };
    bridge.call = vi.fn(async () => ({}));

    await renderBranch();
    await flush();

    expect(document.getElementById("root").textContent).toContain("Desktop isn't connected");
    expect(document.getElementById("tabbody")).toBeNull();
    expect(bridge.call).not.toHaveBeenCalled();
  });

  // The reload case. Two machines answer on their own schedule, and the gate
  // paints as soon as the first one lands: a link to the second paints the
  // notice a beat before its machine is there. The notice is where the link
  // waits, not where it ends — the surface stands itself up the moment that
  // machine can answer, without the reader navigating away and back.
  it("mounts the surface the moment the device lands", async () => {
    const { adoptDeviceSession } = await import("../src/core/deviceContexts.js");
    App.devices = [...App.devices, { id: "dev-3", name: "Laptop", status: "online" }];
    App.route = { name: "branch", deviceId: "dev-3", projectId: "p1", branch: "main", tab: "changes" };
    bridge.call = vi.fn(async () => ({}));
    const theirCall = vi.fn(async (method) => {
      if (method === "branch.get") return { ...row, branch: "main", primary: true, worktree_id: null };
      if (method === "git.status") return { files: [], head: "abc", status_key: "clean" };
      if (method === "git.log") return { commits: [] };
      return {};
    });

    await renderBranch();
    expect(document.getElementById("tabbody")).toBeNull();

    const { adoptBridgeSelection } = await import("../src/core/deviceContexts.js");
    adoptBridgeSelection(
      adoptDeviceSession({ deviceId: "dev-3", call: theirCall, close: () => {}, peer: () => {}, onCarrier: () => {} }),
      { major: 1, version: "1.0.0" },
      {},
    );
    await flush();

    expect(document.getElementById("root").textContent).not.toContain("isn't connected");
    expect(document.getElementById("tabbody")).toBeTruthy();
    expect(theirCall.mock.calls.some(([method]) => method === "branch.get")).toBe(true);
    expect(bridge.call.mock.calls.some(([method]) => method === "branch.get")).toBe(false);
  });

  it("a route naming a device this account has no context for renders the offline state and mounts nothing", async () => {
    App.route = { name: "branch", deviceId: "dev-unknown", projectId: "p1", branch: "main", tab: "changes" };
    bridge.call = vi.fn(async () => ({}));

    await renderBranch();
    await flush();

    expect(document.getElementById("root").textContent).toContain("That device isn't connected");
    expect(document.getElementById("tabbody")).toBeNull();
    expect(bridge.call).not.toHaveBeenCalled();
  });
});

// The way a branch ends. Before this control the only Done was on the inbox
// row, so a branch you were standing in could not be closed out from inside it.
describe("closing the branch out", () => {
  /** Answer the confirmation modal every close-out opens. */
  const answerConfirm = async (ok) => {
    await flush();
    const scrim = document.getElementById("confirm-scrim");
    expect(scrim, "a confirmation was expected").toBeTruthy();
    scrim.querySelector(ok ? "[data-confirm-ok]" : "[data-confirm-cancel]").click();
    await flush();
  };

  const mountWith = async (branchRow, answers = {}) => {
    bridge.call = vi.fn(async (method, params) => {
      if (method === "branch.get") return branchRow;
      if (answers[method]) return answers[method](params);
      return {};
    });
    await renderBranch();
    await flush();
  };

  const finishHost = () => document.querySelector("#tb-verb");
  const doneButton = () => finishHost().querySelector(".btn.mini:not(.caret)");
  const finishCalls = () => bridge.call.mock.calls.filter(([method]) => method === "branch.finish");

  it("offers Done in the surface bar, always pressable — it is never refused", async () => {
    await mountWith(finishableRow({ finish: { warnings: [{ code: "uncommitted", message: "build/login has 2 uncommitted files" }] } }));
    expect(doneButton().textContent).toBe("Done");
    expect(doneButton().disabled).toBe(false);
  });

  // The primary checkout is the repository: there is nothing there to delete.
  it("offers nothing on a project's primary checkout", async () => {
    await mountWith(finishableRow({ primary: true, worktree_id: null }));
    expect(finishHost().innerHTML).toBe("");
  });

  it("offers nothing when the bridge says there is nothing to finish", async () => {
    await mountWith(finishableRow({ can_finish: false }));
    expect(finishHost().innerHTML).toBe("");
  });

  it("deletes the branch, and says so before it does", async () => {
    await mountWith(finishableRow());
    doneButton().click();
    await flush();
    expect(document.getElementById("confirm-scrim").textContent).toContain("Delete branch build/login");
    document.getElementById("confirm-scrim").querySelector("[data-confirm-ok]").click();
    await flush();
    expect(finishCalls()[0][1]).toEqual({ project_id: "p1", branch: "build/login", action: "delete" });
  });

  it("puts the bridge's warnings in the confirmation, above what it will do", async () => {
    await mountWith(
      finishableRow({
        finish: {
          warnings: [
            { code: "unmerged", message: "build/login has never been pushed, and has 3 commits that main does not", count: 3, ref: "main" },
          ],
        },
      }),
    );
    doneButton().click();
    await flush();
    const scrim = document.getElementById("confirm-scrim");
    expect(scrim.querySelector(".confirm-warnings").textContent).toContain("3 commits that main does not");
    expect(scrim.textContent.indexOf("3 commits")).toBeLessThan(scrim.textContent.indexOf("Delete branch"));
    scrim.querySelector("[data-confirm-cancel]").click();
    await flush();
  });

  it("does nothing at all when the confirmation is cancelled", async () => {
    await mountWith(finishableRow());
    doneButton().click();
    await answerConfirm(false);
    expect(finishCalls()).toHaveLength(0);
    // The button comes back: a cancel is not an ending.
    expect(doneButton().disabled).toBe(false);
  });

  // The row poll runs every 1.6s. Re-rendering the Done control on a tick that
  // resolved the same close-out threw away a click already in progress.
  describe("under the row poll", () => {
    beforeEach(() => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
    });
    afterEach(() => {
      vi.useRealTimers();
    });

    const pollTick = async () => {
      await vi.advanceTimersByTimeAsync(2000);
    };

    it("leaves the button standing through a poll that reads the same row", async () => {
      await mountWith(finishableRow());
      const before = doneButton();
      await pollTick();
      expect(doneButton(), "the button was rebuilt by the poll").toBe(before);
    });

    it("holds the busy button through a poll while the deletion is in flight", async () => {
      let finish;
      await mountWith(finishableRow(), {
        "branch.finish": () =>
          new Promise((resolve) => {
            finish = resolve;
          }),
      });
      doneButton().click();
      await answerConfirm(true);
      expect(doneButton().disabled).toBe(true);

      await pollTick();

      expect(doneButton().disabled, "a poll re-armed the button mid-flight").toBe(true);
      finish({ branch: "build/login" });
    });
  });

  it("leaves for the inbox the moment Done is confirmed", async () => {
    await mountWith(finishableRow({ run_id: "run-1" }), { "branch.finish": () => new Promise(() => {}) });
    doneButton().click();
    await answerConfirm(true);
    expect(App.route.name).toBe("inbox");
    expect(location.hash).toContain("inbox");
    expect(finishCalls()[0][1]).toEqual({ project_id: "p1", branch: "build/login", action: "delete" });
  });

  it("keeps the branch's row off the inbox mounted after it", async () => {
    const boardRow = {
      ...finishableRow({ run_id: "run-1" }),
      unread: false,
      unread_count: 0,
      muted: false,
      dismissed: false,
    };
    bridge.call = vi.fn(async (method) => {
      if (method === "branch.get") return boardRow;
      if (method === "board.list") return { items: [boardRow] };
      if (method === "project.list") return { projects: [{ id: "p1", name: "relaydb" }] };
      if (method === "branch.finish") return new Promise(() => {});
      return {};
    });
    await renderBranch();
    await flush();
    doneButton().click();
    await answerConfirm(true);

    const { refreshFeed, stopFeed } = await import("../src/core/taskFeed.js");
    await refreshFeed();
    const { mountInboxList } = await import("../src/core/inboxView.js");
    mountInboxList();
    expect(document.querySelector('#inbox-list .inbox-entry[data-key="run-1"]')).toBeNull();
    stopFeed();
  });

  it("reads the branch it deleted only after the deletion answers", async () => {
    let deleted;
    await mountWith(finishableRow({ run_id: "run-1", issue_id: "iss-9" }), {
      "branch.finish": () =>
        new Promise((resolve) => {
          deleted = resolve;
        }),
    });
    const seenCalls = () => bridge.call.mock.calls.filter(([method]) => method === "entity.seen");
    doneButton().click();
    await answerConfirm(true);
    expect(seenCalls()).toHaveLength(0);

    deleted({ branch: "build/login" });
    await flush();
    expect(seenCalls().map(([, params]) => params.entity_id)).toEqual(["run-1"]);
  });

  it("does not fire a second close-out for a branch the inbox is already finishing", async () => {
    await mountWith(finishableRow({ run_id: "run-1" }), { "branch.finish": () => new Promise(() => {}) });
    const { removeRecord, runOptimistic } = await import("../src/core/optimistic.js");
    const { INBOX_SCOPE } = await import("../src/core/inboxView.js");
    runOptimistic({
      scope: INBOX_SCOPE,
      records: [removeRecord("run-1")],
      call: () => new Promise(() => {}),
      failureSummary: "Couldn't finish build/login",
    });

    doneButton().click();
    await answerConfirm(true);

    expect(finishCalls()).toHaveLength(0);
  });

  it("clears the issue's cursor with the branch when the work landed", async () => {
    let deleted;
    await mountWith(finishableRow({ run_id: "run-1", issue_id: "iss-9", state: "merged" }), {
      "branch.finish": () =>
        new Promise((resolve) => {
          deleted = resolve;
        }),
    });
    const seenCalls = () => bridge.call.mock.calls.filter(([method]) => method === "entity.seen");
    doneButton().click();
    await answerConfirm(true);

    deleted({ branch: "build/login" });
    await flush();

    expect(seenCalls().map(([, params]) => params.entity_id)).toEqual(["run-1", "iss-9"]);
  });

  it("reports a failed close-out as a notice, and does not come back to the branch", async () => {
    await mountWith(finishableRow({ run_id: "run-1" }), {
      "branch.finish": () => {
        throw new Error("worktree.finish cleanup requires no uncommitted changes");
      },
    });
    doneButton().click();
    await answerConfirm(true);
    const notices = [...document.querySelectorAll("#notices .notice.error")];
    expect(notices).toHaveLength(1);
    expect(notices[0].textContent).toContain("build/login");
    expect(notices[0].textContent).toContain("worktree.finish cleanup requires no uncommitted changes");
    expect(App.route.name).toBe("inbox");
  });
});
