// @vitest-environment jsdom
// The project's rail, wired for real (#274): the cells core/directoryRail.js
// draws, and the Tasks count read off the project's cached task list — the
// unread of every watched task in the project (#104), painted from the cache
// with no machine asked.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, task } from "./trackerWireFixture.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const openProjectSettings = vi.fn();
vi.mock("../src/sheets/projectSettings.js", () => ({ openProjectSettings: (...args) => openProjectSettings(...args) }));

const { mountProjectRail } = await import("../src/core/projectRail.js");
const { writeTasksRecord } = await import("../src/core/trackerCache.js");
const { wipeCache } = await import("../src/core/localCache.js");

const route = { name: "project", deviceId: "dev-1", projectId: "p-1" };
const context = { deviceId: "dev-1", rpc: vi.fn() };

let rail, selected;
const host = () => document.querySelector("#dir-rail");
const count = () => host().querySelector("[data-tab=tasks] .dirtab-count").textContent;

beforeEach(async () => {
  await wipeCache();
  document.body.innerHTML = '<nav id="dir-rail"></nav>';
  openProjectSettings.mockClear();
  selected = [];
  rail = mountProjectRail(host(), { route, context, onSelect: (tab) => selected.push(tab), navigate: vi.fn() });
});

afterEach(() => rail.dispose());

describe("the project rail, wired", () => {
  it("counts the project's watched unread on the Tasks icon, across repaints and record moves", async () => {
    await writeTasksRecord("dev-1", "p-1", {
      tasks: [
        task({ number: 1, id: "i1", status: "in_progress", watched: true, unread_count: 2 }),
        task({ number: 2, id: "i2", status: "ready", unread_count: 9 }),
        task({ number: 3, id: "i3", status: "done", watched: true, unread_count: 4 }),
      ],
      columns: columns(),
    });
    rail.paint("tasks");
    await vi.waitFor(() => expect(count()).toBe("2"));

    rail.paint("workspaces");
    expect(count()).toBe("2");

    await writeTasksRecord("dev-1", "p-1", { tasks: [task({ number: 1, id: "i1", watched: true, unread_count: 0 })], columns: columns() });
    await vi.waitFor(() => expect(count()).toBe(""));
  });

  it("hands a press on a face to the surface", () => {
    rail.paint("tasks");
    host().querySelector("[data-tab=workspaces]").click();
    expect(selected).toEqual(["workspaces"]);
  });

  it("opens the project's settings on the route's machine", () => {
    rail.paint("tasks");
    host().querySelector("[data-rail-settings]").click();
    expect(openProjectSettings).toHaveBeenCalledWith("p-1", expect.objectContaining({ callRpc: context.rpc, deviceId: "dev-1" }));
    expect(selected).toEqual([]);
  });

  // A task's page is a page OF the Tasks face, so an arrow there navigates to
  // the project page, and the app takes the task page down before it stands the
  // project page up on the same column (app.js renderPage). The keyboard was on
  // the rail and stays on it, the way it does when a workspace's rail switches
  // faces in place.
  it("keeps the keyboard on the rail across a navigation to the face arrowed to", () => {
    let next = null;
    rail.dispose();
    rail = mountProjectRail(host(), {
      route,
      context,
      navigate: vi.fn(),
      onSelect: (tab) => {
        next = tab;
      },
    });
    rail.paint("tasks");
    host().querySelector("[data-tab=tasks]").focus();
    document.activeElement.dispatchEvent(new window.KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    expect(next).toBe("workspaces");

    rail.dispose();
    rail = mountProjectRail(host(), { route, context, onSelect: (tab) => selected.push(tab), navigate: vi.fn() });
    rail.paint(next);
    expect(document.activeElement).toBe(host().querySelector("[data-tab=workspaces]"));
  });

  it("hands the column back empty when the keyboard was on it and nothing stands up after", async () => {
    rail.paint("tasks");
    host().querySelector("[data-tab=tasks]").focus();
    rail.dispose();
    await Promise.resolve();
    expect(host().children).toHaveLength(0);
  });

  it("hands the shell's column back empty when the surface leaves", () => {
    rail.paint("tasks");
    rail.dispose();
    expect(host().children).toHaveLength(0);
  });
});
