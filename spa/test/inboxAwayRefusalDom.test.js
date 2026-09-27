// @vitest-environment jsdom
// #140: nothing on the rail is shut because its machine is away. Every control
// a row or block has stays live, and the press is what finds out: each one is
// refused at the moment it acts, in one plain sentence on the surface that was
// pressed — "Build cannot … because this machine is away.", or the actual
// reason when the machine is not away but blocked or behind. Which it is, is
// read at the press: a sheet opened while the machine was away sends once it is
// back, and one opened while it was here refuses once it has gone.
//
// Real cache, feed, device registry, rail, create dialog and settings sheet;
// the session's `call` is the only stand-in, and a machine that is away never
// reaches it.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { task, taskDetail } from "./trackerWireFixture.js";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
const DEVICE = "dev-1";
const PROJECT = "proj-1";
const PROJECT_KEY = `${DEVICE}/${PROJECT}`;
const WAIT = { timeout: 5000, interval: 20 };
const GREETING = {
  api_version: "2.0.0",
  push_events: true,
  capabilities: ["tasks.watching", "tasks.attachments"],
  tasks: { watching: true, attachments: true },
};
const project = { project_id: PROJECT, name: "Build", path: "/work/build", sources: [{ id: "src-1", name: "api", path: "/work/api" }] };
const workspace = {
  id: "ws-1",
  project_id: PROJECT,
  name: "Checkout",
  root: "/work/checkout",
  status: "ready",
  can_finish: true,
  finish_blockers: [],
  directories: [{ id: "api", source_id: "src-1", is_git: true }],
};
const watchedTask = task({ id: "task-7", number: 7, title: "Wire 1.22", watched: true, status: "in_review", updated_at: "2026-09-24T01:00:00Z" });

let modules;
/** The bridge: greets, and answers every verb — so a verb that reached it would
 *  succeed, and a sentence on screen can only have come from the refusal. */
const answerAll = (method) => Promise.resolve(method === "session.hello" ? GREETING : {});
const call = vi.fn(answerAll);
/** A new session on the machine, greeted: how it comes back. */
async function connect() {
  const session = vi.fn((method, params) => call(method, params));
  const context = modules.deviceContexts.adoptDeviceSession({
    deviceId: DEVICE, call: session, close: () => {}, peer: () => {}, onCarrier: () => {},
    installAdapter: (selection) => selection.create(session),
  });
  await modules.connection.greetLiveBridge(context);
  return context;
}
const sentVerbs = () => call.mock.calls.map(([method]) => method).filter((method) => method !== "session.hello");

const $ = (selector) => document.querySelector(selector);
const block = () => $(`[data-project="${PROJECT_KEY}"]`);
const rowFor = (key) => [...document.querySelectorAll("#inbox-list .inbox-entry")].find((row) => row.dataset.key === key) || null;
const rowError = (key) => rowFor(key)?.querySelector("[data-done-error]");
const shown = (element) => Boolean(element && !element.hidden && element.textContent);

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = bodyHtml;
  call.mockClear();
  call.mockImplementation(answerAll);
  const { App } = await import("../src/app.js");
  Object.assign(App, { route: { name: "inbox" }, devices: [{ id: DEVICE, name: "Laptop", status: "online" }],
    selectedDeviceId: DEVICE, deviceFilter: null });
  modules = {
    tracker: await import("../src/core/trackerCache.js"),
    taskFeed: await import("../src/core/taskFeed.js"),
    inboxView: await import("../src/core/inboxView.js"),
    deviceContexts: await import("../src/core/deviceContexts.js"),
    connection: await import("../src/connection.js"),
  };
  // A reload: the board and the watched task are already on disk. The fixture
  // is imported with the fresh modules, so it writes the cache they read.
  const { writeRailBoard } = await import("./railCacheFixture.js");
  await writeRailBoard({ projects: [project], workspaces: [workspace] });
  await modules.tracker.writeTasksRecord(DEVICE, PROJECT, modules.tracker.tasksRecord([watchedTask], []));
  await modules.tracker.writeTaskRecord(DEVICE, PROJECT, watchedTask.id, taskDetail(watchedTask, []));
  // The machine answered once — which is how this tab knows it carries
  // watching — and is away now.
  await connect();
  modules.deviceContexts.setContextOffline(DEVICE, { offline: true });
  modules.inboxView.setInboxView("projects");
  modules.inboxView.mountInboxList();
  await modules.taskFeed.startFeed();
  await vi.waitFor(() => expect(block()?.classList.contains("inbox-offline")).toBe(true), WAIT);
  call.mockClear();
});

afterEach(() => {
  modules.inboxView.unmountInboxList();
  modules.taskFeed.stopFeed();
  modules.deviceContexts.resetDeviceContexts();
});

describe("a press on a machine that is away", () => {
  it("says why a workspace's Done did nothing, on the row", async () => {
    const key = `workspace:${DEVICE}/ws-1`;
    await vi.waitFor(() => expect(rowFor(key)?.querySelector("[data-workspace-done]")).toBeTruthy(), WAIT);
    rowFor(key).querySelector("[data-workspace-done]").click();
    await vi.waitFor(() => expect(shown(rowError(key))).toBe(true), WAIT);
    expect(rowError(key).textContent).toBe("Build cannot archive this workspace because this machine is away.");
    expect(sentVerbs()).toEqual([]);
  });

  it("says why Stop watching did nothing, on the task's row", async () => {
    const key = `tracker_task:${watchedTask.id}`;
    await vi.waitFor(() => expect(rowFor(key)).not.toBe(null), WAIT);
    rowFor(key).querySelector("[data-menu]").click();
    await vi.waitFor(() => expect(rowFor(key).querySelector("[data-unwatch]")).not.toBe(null), WAIT);
    rowFor(key).querySelector("[data-unwatch]").click();
    await vi.waitFor(() => expect(shown(rowError(key))).toBe(true), WAIT);
    expect(rowError(key).textContent).toBe("Build cannot stop watching this task because this machine is away.");
    const held = await modules.tracker.readTasksRecord(DEVICE, PROJECT);
    expect(held.tasks[0].watched).toBe(true);
    expect(sentVerbs()).toEqual([]);
  });

  it("says why a new workspace was not made, in the create dialog", async () => {
    block().querySelector("[data-project-create]").click();
    await vi.waitFor(() => expect($("[data-create-go]")).not.toBe(null), WAIT);
    $("#create-work-input").value = "fix";
    $("#create-work-input").dispatchEvent(new Event("input"));
    $("[data-create-go]").click();
    await vi.waitFor(() => expect(shown($(".create-error"))).toBe(true), WAIT);
    expect($(".create-error").textContent).toBe("Build cannot create a workspace because this machine is away.");
    expect(sentVerbs()).toEqual([]);
  });

  it("says why settings nothing cached could not be opened, in the sheet", async () => {
    block().querySelector("[data-project-settings]").click();
    await vi.waitFor(() => expect($("#sheet").textContent).toContain("because this machine is away"), WAIT);
    expect($("#sheet .sub").textContent).toBe("Build cannot open this project's settings because this machine is away.");
  });

  describe("in a project's settings, painted from the cache", () => {
    beforeEach(async () => {
      const { writeProjectSetting } = await import("../src/core/settingsRecords.js");
      await writeProjectSetting(DEVICE, project);
      block().querySelector("[data-project-settings]").click();
      await vi.waitFor(() => expect($("#pssave")).not.toBe(null), WAIT);
    });

    it("says why the remote was not saved", async () => {
      $("#psremote").value = "git@example.com:build.git";
      $("#pssave").click();
      await vi.waitFor(() => expect($("#pserr").textContent).not.toBe(""), WAIT);
      expect($("#pserr").textContent).toBe("Build cannot save this project's remote because this machine is away.");
      expect(sentVerbs()).toEqual([]);
    });

    it("sends the remote once the machine is back, though the sheet opened while it was away", async () => {
      await connect();
      await vi.waitFor(() => expect(block().classList.contains("inbox-offline")).toBe(false), WAIT);
      $("#psremote").value = "git@example.com:build.git";
      $("#pssave").click();
      await vi.waitFor(() => expect(sentVerbs()).toContain("project.set_remote"), WAIT);
      expect(call).toHaveBeenCalledWith("project.set_remote", { project_id: PROJECT, url: "git@example.com:build.git" });
      expect($("#pserr").textContent).toBe("");
    });

    it("says why a folder was not removed", async () => {
      $('[data-remove-source="src-1"]').click();
      await vi.waitFor(() => expect($("#pssrcerr").textContent).not.toBe(""), WAIT);
      expect($("#pssrcerr").textContent).toBe("Build cannot remove this folder from this project because this machine is away.");
      expect(sentVerbs()).toEqual([]);
    });

    it("says why the project was not deleted", async () => {
      $("#psdelete").click();
      await vi.waitFor(() => expect($("[data-confirm-ok]")).not.toBe(null), WAIT);
      $("[data-confirm-ok]").click();
      await vi.waitFor(() => expect($("#pserr").textContent).not.toBe(""), WAIT);
      expect($("#pserr").textContent).toBe("Build cannot delete this project because this machine is away.");
      expect(sentVerbs()).toEqual([]);
    });
  });

  it("says why the remote was not saved when the machine went while the sheet was open", async () => {
    const { writeProjectSetting } = await import("../src/core/settingsRecords.js");
    await writeProjectSetting(DEVICE, project);
    await connect();
    await vi.waitFor(() => expect(block().classList.contains("inbox-offline")).toBe(false), WAIT);
    block().querySelector("[data-project-settings]").click();
    await vi.waitFor(() => expect($("#pssave")).not.toBe(null), WAIT);
    modules.deviceContexts.setContextOffline(DEVICE, { offline: true });
    call.mockClear();
    $("#psremote").value = "git@example.com:build.git";
    $("#pssave").click();
    await vi.waitFor(() => expect($("#pserr").textContent).not.toBe(""), WAIT);
    expect($("#pserr").textContent).toBe("Build cannot save this project's remote because this machine is away.");
    expect(sentVerbs()).toEqual([]);
  });

  it("says the machine went, when it goes while the press is in flight", async () => {
    await connect();
    await vi.waitFor(() => expect(block().classList.contains("inbox-offline")).toBe(false), WAIT);
    const key = `workspace:${DEVICE}/ws-1`;
    let drop;
    call.mockImplementation((method) => (method === "workspace.finish"
      ? new Promise((_, reject) => { drop = reject; })
      : answerAll(method)));
    await vi.waitFor(() => expect(rowFor(key)?.querySelector("[data-workspace-done]")).toBeTruthy(), WAIT);
    rowFor(key).querySelector("[data-workspace-done]").click();
    await vi.waitFor(() => expect(drop).toBeTypeOf("function"), WAIT);
    modules.deviceContexts.setContextOffline(DEVICE, { offline: true });
    drop(new Error("Device offline"));
    await vi.waitFor(() => expect(shown(rowError(key))).toBe(true), WAIT);
    expect(rowError(key).textContent).toBe("Build cannot archive this workspace because this machine is away.");
  });

  describe("on a machine that is not away", () => {
    const pressDone = async () => {
      const key = `workspace:${DEVICE}/ws-1`;
      await vi.waitFor(() => expect(rowFor(key)?.querySelector("[data-workspace-done]")).toBeTruthy(), WAIT);
      rowFor(key).querySelector("[data-workspace-done]").click();
      await vi.waitFor(() => expect(shown(rowError(key))).toBe(true), WAIT);
      return rowError(key).textContent;
    };

    it("says this browser could not connect to it, when it is blocked", async () => {
      const { blockedMark } = await import("../src/core/deviceAway.js");
      modules.deviceContexts.setContextOffline(DEVICE, blockedMark("no-webrtc"));
      expect(await pressDone()).toBe(
        "Build cannot archive this workspace because a direct connection to this machine could not be made: this browser cannot open one.",
      );
      expect(sentVerbs()).toEqual([]);
    });

    it("says this app needs a reload, when the machine's bridge is newer", async () => {
      // What a greeting in an API major nothing here speaks settles.
      const context = await connect();
      modules.deviceContexts.adoptBridgeSelection(context, { unsupported: "app", version: "2.0.0" });
      call.mockClear();
      expect(await pressDone()).toBe(
        "Build cannot archive this workspace because this machine's bridge is newer than this app, which needs a reload.",
      );
      expect(sentVerbs()).not.toContain("workspace.finish");
    });
  });

  // Done off a row's menu and off a branch's own page both finish through
  // finishWorkItem, and the page shows what it throws.
  it("says why Done on a task or a branch did nothing", async () => {
    const { finishWorkItem } = modules.inboxView;
    await expect(finishWorkItem({ kind: "task", deviceId: DEVICE, taskId: "task-7" })).rejects.toThrow(
      "Build cannot archive this task because this machine is away.",
    );
    // #87: this machine's bridge keeps the branch, so Done there removes only
    // the checkout, and the refusal names that.
    await expect(finishWorkItem({ kind: "branch", deviceId: DEVICE, projectId: PROJECT, branch: "fix" })).rejects.toThrow(
      "Build cannot remove this checkout because this machine is away.",
    );
    expect(sentVerbs()).toEqual([]);
  });

  // Hide asks no machine anything, so it is the one press that works while the
  // machine is away — from the block's menu, as on every block.
  it("hides the block from its menu", async () => {
    block().querySelector(".inbox-project-head [data-menu]").click();
    await vi.waitFor(() => expect(block().querySelector("[data-project-hide]")).not.toBe(null), WAIT);
    block().querySelector("[data-project-hide]").click();
    await vi.waitFor(() => expect(block()).toBe(null), WAIT);
  });
});
