// @vitest-environment jsdom
// How far the composer's model and reasoning menus may reach.
//
// The box they hang from is the panel's foot — three rows tall, with the
// conversation above it — so both menus open UPWARD, over the thread. A model
// name is wider than the button that states it, so on a phone the menu reaches
// left of the composer's own edge as well.
//
// core/splitButton.js lifts a menu inside a scroller to `position:fixed`, which
// takes it out of the FLOW but not out of a clip: an ancestor that hides its
// overflow still cuts a fixed descendant wherever that ancestor is its
// containing block, which the composer is on an engine that reads its glass
// (-webkit-backdrop-filter) that way. So the boxes between the menu and the
// panel must not clip at all.

import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const bridge = { call: null };

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
const shellCss = readFileSync(resolve("src/styles/shell.css"), "utf8");

vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: () => () => {},
  startFeed: () => {},
  stopFeed: () => {},
  refreshFeed: async () => [],
}));
vi.mock("../src/core/inboxView.js", () => ({
  markSeen: async () => {},
  noteSelfAction: async () => {},
  mountInboxList: () => {},
  inboxListRouteChanged: () => {},
}));
vi.mock("../src/core/notify.js", () => ({ notifyError: () => {}, notifySuccess: () => {} }));
vi.mock("../src/core/surfaceTabs.js", () => ({ mountAgentTab: () => ({ dispose: () => {} }) }));

await import("../src/app.js");
const { mountAgentRail, resetAgentRailMemory } = await import("../src/core/agentRail.js");
const { mountComposerModelMenu } = await import("../src/core/composer.js");
const { wipeCache } = await import("../src/core/localCache.js");
const { writeRailWorkItem } = await import("./railCacheFixture.js");

const CATALOG = {
  default_provider: "claude",
  providers: [{
    id: "claude",
    label: "Claude Code",
    models: [
      { id: "claude-opus-5", label: "Claude Opus 5", supports_effort: true },
      { id: "claude-haiku-4-5", label: "Claude Haiku 4.5", supports_effort: true },
    ],
    efforts: ["low", "high"],
  }],
};

const branchRow = () => ({
  kind: "branch",
  project_id: "p1",
  branch: "build/login",
  run_id: "run-3",
  worktree_id: "wt-3",
  agents: [{ id: "ag-1", ordinal: 1, provider: "claude", state: "live", unread_count: 0, working: false }],
  run: { run_id: "run-3", thread: { items: [], sessions: [] } },
});

let rail = null;

const panel = () => document.getElementById("rail-panel");

/** The rail's own composer under ag-1's conversation, with both menus painted
 *  on it. The rail reads its agents off the cache, so the row goes there, and
 *  the agent's bubble says the conversation is up rather than the new-agent
 *  panel (#258). The rail asks its device for the catalog and this mount has no
 *  device, so the menus are hung here the way the rail hangs them once its
 *  answer lands. */
const mountRailWithMenus = async () => {
  await writeRailWorkItem(branchRow());
  rail = mountAgentRail(document.getElementById("agent-rail"), {
    kind: "branch",
    deviceId: "dev-1",
    projectId: "p1",
    branch: "build/login",
    call: (method, params) => bridge.call(method, params),
  });
  await vi.waitFor(() => {
    expect(document.querySelector('#agent-rail [data-bubble="agent"][data-agent="ag-1"]')).toBeTruthy();
    expect(panel()?.querySelector("#railinputmodel")).toBeTruthy();
  });
  mountComposerModelMenu(panel(), {
    ids: { input: "railinput", send: "railsend", hint: "railhint" },
    onChoose: () => {},
  }).set(CATALOG, "claude", { provider: "claude", model: "claude-haiku-4-5", effort: "high" });
};

/** Every box between a menu and the panel — what has to let it out. */
const boxesAroundMenu = (menu) => {
  const chain = [];
  for (let box = menu.parentElement; box && box !== panel(); box = box.parentElement) chain.push(box);
  return chain;
};

const classesAround = (selector) =>
  boxesAroundMenu(panel().querySelector(`${selector} .splitmenu`)).map((box) => box.className.trim());

beforeEach(async () => {
  document.body.innerHTML = bodyHtml;
  localStorage.clear();
  resetAgentRailMemory();
  await wipeCache();
  bridge.call = vi.fn(async () => ({}));
});

afterEach(() => {
  if (rail) rail.dispose();
  rail = null;
});

describe("the menus on the composer's row", () => {
  it("hangs both of them inside the composer's own box", async () => {
    await mountRailWithMenus();
    for (const selector of ["#railinputmodel", "#railinputreasoning"]) {
      expect(panel().querySelector(`${selector} .splitmenu`)).toBeTruthy();
      expect(classesAround(selector)).toEqual(
        expect.arrayContaining(["splitbtn", "composer attachable", "thread-composer"]),
      );
    }
  });
});

describe("the rail composer's stylesheet", () => {
  const threadComposerRule = () => shellCss.match(/\.rail-composer \.thread-composer \{[^}]*\}/)[0];
  const composerBoxRule = () => shellCss.match(/\.rail-composer \.composer \{[^}]*\}/)[0];
  const composerTextRule = () => shellCss.match(/\.rail-composer \.composer textarea \{[^}]*\}/)[0];

  // Both boxes are flex columns whose rows scroll themselves (the tray, the
  // text), so neither needs to hide what is inside it — and hiding it is what
  // cut the model menu off at the composer's left edge.
  it("lets a menu out of the boxes it hangs from", () => {
    expect(threadComposerRule()).not.toMatch(/overflow:(hidden|clip|auto|scroll)/);
    expect(composerBoxRule()).not.toMatch(/overflow:(hidden|clip|auto|scroll)/);
  });

  // The rows inside still scroll rather than growing without end.
  it("keeps the rows that scroll scrolling", () => {
    expect(composerTextRule()).toMatch(/overflow-y:auto/);
    expect(shellCss).toMatch(/\.rail-composer \.composer-context,\.rail-composer \.composer-tray\{[^}]*overflow-y:auto\}/);
  });
});
