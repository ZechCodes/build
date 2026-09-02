// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SPAWNING_CALL_SEQUENCE, surfacesSnapshot } from "./surfacesFixture.js";
import { mountSurfaceViewer } from "../src/core/agentSurfaces.js";
import {
  AGENT_ENTRY_KIND,
  SHELL_ENTRY_KIND,
  WORKFLOW_ENTRY_KIND,
  rowActions,
  surfaceRows,
} from "../src/core/agentSurfacesModel.js";

const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args), notify: () => {} }));

const snapshot = () => surfacesSnapshot();

const host = () => {
  document.body.innerHTML = `<div class="overlay-host"></div>`;
  return document.querySelector(".overlay-host");
};

const mount = (kind, options = {}) =>
  mountSurfaceViewer(host(), kind, {
    onSendMessage: options.onSendMessage || (async () => {}),
    onOpenThreadItem: options.onOpenThreadItem || (() => {}),
  });

const runningRows = () => [...document.querySelectorAll(".surface-running > .surface-row")];

const chooseRowAction = (row, actionId) => {
  row.querySelector(".caret").click();
  row.querySelector(`.mi[data-action="${actionId}"]`).click();
};

const shellRowAction = () => rowActions(SHELL_ENTRY_KIND, surfaceRows(SHELL_ENTRY_KIND, snapshot())[0])[0];

beforeEach(() => notifyError.mockClear());

describe("mountSurfaceViewer", () => {
  it("paints the kind it was mounted for, with its rows and their Ask menus", () => {
    const viewer = mount(SHELL_ENTRY_KIND);
    viewer.set(snapshot());

    expect(document.querySelector(".surface-shells")).not.toBe(null);
    expect(runningRows().map((row) => row.dataset.key)).toEqual(["sh1"]);
    expect(runningRows()[0].querySelector(".caret")).not.toBe(null);
    viewer.dispose();
  });

  it("paints its frame with no rows for a kind the snapshot has nothing of", () => {
    const viewer = mount(SHELL_ENTRY_KIND);
    viewer.set({ subagents: snapshot().subagents });

    expect(document.querySelector(".surface-shells")).not.toBe(null);
    expect(runningRows()).toEqual([]);
    viewer.dispose();
  });

  it("adds the row a later snapshot brought, keeping the one already painted", () => {
    const viewer = mount(SHELL_ENTRY_KIND);
    viewer.set(snapshot());
    const [first] = runningRows();

    const grown = snapshot();
    grown.shells.push({ id: "sh2", description: "cargo clippy", state: "running", tail: [] });
    viewer.set(grown);

    expect(runningRows()).toHaveLength(2);
    expect(runningRows()[0]).toBe(first);
    expect(runningRows()[1].textContent).toContain("cargo clippy");
    viewer.dispose();
  });

  it("sends the message the chosen row action named", () => {
    const onSendMessage = vi.fn(async () => {});
    const viewer = mount(SHELL_ENTRY_KIND, { onSendMessage });
    viewer.set(snapshot());
    const action = shellRowAction();

    chooseRowAction(runningRows()[0], action.id);

    expect(onSendMessage.mock.calls).toEqual([[action.message]]);
    viewer.dispose();
  });

  it("says so when the agent refuses the message, and leaves the rows where they were", async () => {
    const onSendMessage = vi.fn(async () => {
      throw new Error("the agent is gone");
    });
    const viewer = mount(SHELL_ENTRY_KIND, { onSendMessage });
    viewer.set(snapshot());

    chooseRowAction(runningRows()[0], shellRowAction().id);
    await Promise.resolve();
    await Promise.resolve();

    expect(notifyError.mock.calls).toEqual([["Could not ask the agent", "the agent is gone"]]);
    expect(runningRows().map((row) => row.dataset.key)).toEqual(["sh1"]);
    viewer.dispose();
  });

  it("chooses the phase pressed in a workflow, without leaving the viewer", () => {
    const viewer = mount(WORKFLOW_ENTRY_KIND);
    viewer.set(snapshot());
    const [, secondPhase] = [...document.querySelectorAll(".surface-phase")];

    secondPhase.click();

    expect(secondPhase.getAttribute("aria-pressed")).toBe("true");
    expect(document.querySelector(".surface-phase-agents").textContent).toContain("judge");
    viewer.dispose();
  });

  it("opens the thread item a subagent row was spawned by", () => {
    const onOpenThreadItem = vi.fn();
    const viewer = mount(AGENT_ENTRY_KIND, { onOpenThreadItem });
    viewer.set(snapshot());

    document.querySelector(".surface-completed-rows > .surface-row").click();

    expect(onOpenThreadItem.mock.calls).toEqual([[SPAWNING_CALL_SEQUENCE]]);
    viewer.dispose();
  });

  it("empties its host on dispose and hears nothing more from it", () => {
    const onOpenThreadItem = vi.fn();
    const viewer = mount(AGENT_ENTRY_KIND, { onOpenThreadItem });
    viewer.set(snapshot());
    const [row] = [...document.querySelectorAll(".surface-completed-rows > .surface-row")];
    const mountedHost = document.querySelector(".overlay-host");

    viewer.dispose();

    expect(mountedHost.innerHTML).toBe("");
    mountedHost.appendChild(row);
    row.click();
    expect(onOpenThreadItem).not.toHaveBeenCalled();
  });
});
