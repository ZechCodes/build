// @vitest-environment jsdom
// The model and reasoning menus on the composer's left, opposite the send.
//
// It is the split button's menu half and nothing else: one button that says
// what the next turn will run on, and a menu of the models the agent's own
// harness offers plus how hard to make it think. No harness question — an agent
// is locked to the one it was created on.
//
// A live session is untouched by any of this. The choice is what the NEXT start
// spends, which is why the menu can be pressed mid-turn without stopping one.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { composerHtml, composerPartIds, mountComposerModelMenu } from "../src/core/composer.js";
import { readCached, wipeCache, writeCached } from "../src/core/localCache.js";
import { uiAddress } from "../src/core/localUiState.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const IDS = { input: "ti", send: "ts", hint: "th" };
const CATALOG = {
  default_provider: "claude_adk",
  providers: [
    {
      id: "claude_adk",
      label: "Claude Code",
      models: [
        { id: "claude-opus-5", label: "Claude Opus 5", supports_effort: true },
        { id: "claude-haiku-4-5", label: "Claude Haiku 4.5", supports_effort: false },
      ],
      efforts: ["low", "high"],
    },
    { id: "codex", label: "Codex", models: [{ id: "gpt", label: "GPT", supports_effort: true }], efforts: ["medium"] },
  ],
};

const host = () => document.querySelector("#host");
const slot = () => host().querySelector(`#${composerPartIds(IDS.input).modelMenu}`);
const reasoningSlot = () => host().querySelector(`#${composerPartIds(IDS.input).reasoningMenu}`);
const button = () => slot().querySelector(".caret");
const reasoningButton = () => reasoningSlot().querySelector(".caret");
const menu = () => slot().querySelector(".splitmenu");
const reasoningMenu = () => reasoningSlot().querySelector(".splitmenu");
const items = () => [...menu().querySelectorAll(".mi")].map((item) => item.dataset.action);
const reasoningItems = () => [...reasoningMenu().querySelectorAll(".mi")].map((item) => item.dataset.action);
const item = (action) => menu().querySelector(`.mi[data-action="${action}"]`);
const reasoningItem = (action) => reasoningMenu().querySelector(`.mi[data-action="${action}"]`);

const mount = (choice, { provider = "claude_adk", onChoose = vi.fn(), activeModel = "", activeEffort = "", cacheKey = null } = {}) => {
  document.body.innerHTML = `<div id="host">${composerHtml({
    inputId: IDS.input, sendId: IDS.send, hintId: IDS.hint, placeholder: "…", modelMenu: true,
  })}</div>`;
  const control = mountComposerModelMenu(host(), { ids: IDS, onChoose, cacheKey });
  control.set(CATALOG, provider, choice, activeModel, activeEffort);
  return { control, onChoose };
};

beforeEach(() => {
  document.body.innerHTML = "";
});

describe("where the menu sits", () => {
  it("restores and redraws an open conversation menu from real cache writes", async () => {
    await wipeCache();
    const address = uiAddress({ entityId: "conversation-menu", view: "composer", kind: "menu", sub: "model" });
    await writeCached(address, { open: "model" });
    const { control } = mount({ provider: "claude_adk", model: "", effort: "" }, { cacheKey: "conversation-menu" });
    await control.ready;
    expect(menu().hidden).toBe(false);

    await writeCached(address, { open: "reasoning" });
    await vi.waitFor(() => expect(reasoningMenu().hidden).toBe(false));
    expect(menu().hidden).toBe(true);

    reasoningButton().click();
    await vi.waitFor(async () => expect((await readCached(address))?.value?.open).toBeNull());
    control.dispose();
  });

  it("is on the row's left, opposite the send", () => {
    const html = composerHtml({ inputId: "i", sendId: "s", hintId: "h", placeholder: "p", modelMenu: true });
    const bar = html.slice(html.indexOf('class="composer-bar"'));
    expect(bar.indexOf("composer-model")).toBeLessThan(bar.indexOf("composer-actions"));
    expect(bar).toContain('id="imodel"');
    expect(bar).toContain('id="ireasoning"');
  });

  it("is absent from a composer that does not ask for it", () => {
    const html = composerHtml({ inputId: "i", sendId: "s", hintId: "h", placeholder: "p" });
    expect(html).not.toContain("composer-model");
  });

  it("lifts a clipped menu at a bounded viewport position instead of stretching it from the left edge", () => {
    mount({ provider: "claude_adk", model: "claude-opus-5", effort: "high" });
    host().style.overflowY = "hidden";
    slot().querySelector(".splitbtn").getBoundingClientRect = () => ({ top: 300, bottom: 330, left: 900, right: 1020 });
    Object.defineProperties(menu(), {
      offsetWidth: { configurable: true, value: 180 },
      offsetHeight: { configurable: true, value: 80 },
    });
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 1200 });

    button().click();

    expect(menu().style.position).toBe("fixed");
    expect(menu().style.left).toBe("840px");
    expect(menu().style.right).toBe("auto");
  });
});

describe("what the menu offers", () => {
  it("shows separate model and reasoning controls with their own choices", () => {
    mount({ provider: "claude_adk", model: "claude-opus-5", effort: "high" });
    expect(button().textContent).toContain("Claude Opus 5");
    expect(reasoningButton().textContent).toContain("high");
    expect(button().textContent).not.toContain("▾");
    expect(reasoningButton().textContent).not.toContain("▾");
    button().click();
    expect(items()).toEqual(["model:claude-opus-5", "model:claude-haiku-4-5"]);
    reasoningButton().click();
    expect(reasoningItems()).toEqual(["effort:low", "effort:high"]);
    expect(reasoningMenu().textContent).not.toContain("reasoning effort");
    expect(item("model:claude-opus-5").className).toContain("on");
    expect(reasoningItem("effort:high").className).toContain("on");
  });

  it("offers the models of the harness the agent is on, and no harness of its own", () => {
    mount({ provider: "codex", model: "", effort: "" }, { provider: "codex" });
    expect(button().textContent).toContain("Harness default");
    expect(reasoningButton().textContent).toContain("Default effort");
    button().click();
    expect(items()).toEqual(["model:gpt"]);
    reasoningButton().click();
    expect(reasoningItems()).toEqual(["effort:medium"]);
    expect(items().some((action) => action.startsWith("provider"))).toBe(false);
  });

  it("drops the effort question for a model that does not answer it", () => {
    mount({ provider: "claude_adk", model: "claude-haiku-4-5", effort: "" });
    button().click();
    expect(items()).toEqual(["model:claude-opus-5", "model:claude-haiku-4-5"]);
    expect(reasoningSlot().hidden).toBe(true);
  });
});

describe("choosing", () => {
  it("reports the whole choice, and says it on the button", () => {
    const { onChoose } = mount({ provider: "claude_adk", model: "", effort: "" });
    button().click();
    item("model:claude-opus-5").click();

    expect(onChoose).toHaveBeenCalledWith({ provider: "claude_adk", model: "claude-opus-5", effort: "" });
    expect(button().textContent).toContain("Claude Opus 5");
    expect(menu().hidden).toBe(true);
  });

  it("drops an effort the newly chosen model cannot take", () => {
    const { onChoose } = mount({ provider: "claude_adk", model: "claude-opus-5", effort: "high" });
    button().click();
    item("model:claude-haiku-4-5").click();
    expect(onChoose).toHaveBeenCalledWith({ provider: "claude_adk", model: "claude-haiku-4-5", effort: "" });
  });

  it("changes reasoning independently of the selected model", () => {
    const { onChoose } = mount({ provider: "claude_adk", model: "claude-opus-5", effort: "low" });
    reasoningButton().click();
    reasoningItem("effort:high").click();

    expect(onChoose).toHaveBeenCalledWith({ provider: "claude_adk", model: "claude-opus-5", effort: "high" });
    expect(reasoningButton().textContent).toContain("high");
  });
});

describe("a repaint under the poll", () => {
  it("leaves the open menu alone when nothing about the choice moved", () => {
    const { control } = mount({ provider: "claude_adk", model: "claude-opus-5", effort: "" });
    button().click();
    const opened = menu();
    expect(opened.hidden).toBe(false);

    control.set(CATALOG, "claude_adk", { provider: "claude_adk", model: "claude-opus-5", effort: "" });

    expect(menu()).toBe(opened);
    expect(menu().hidden).toBe(false);
  });

  it("repaints when the choice moved under it — another device chose", () => {
    const { control } = mount({ provider: "claude_adk", model: "", effort: "" });
    control.set(CATALOG, "claude_adk", { provider: "claude_adk", model: "claude-opus-5", effort: "low" });
    expect(button().textContent).toContain("Claude Opus 5");
    expect(reasoningButton().textContent).toContain("low");
  });
});

describe("the model the agent is running on", () => {
  it("shows the reported reasoning level when no override is selected", () => {
    mount(
      { provider: "claude_adk", model: "", effort: "" },
      { activeModel: "claude-opus-5", activeEffort: "high" },
    );
    expect(reasoningButton().textContent).toContain("high");
    reasoningButton().click();
    expect(reasoningItem("effort:high").className).toContain("on");
  });

  it("does not pass off the old session's effort as the pending model's level", () => {
    mount(
      { provider: "claude_adk", model: "claude-opus-5", effort: "" },
      { activeModel: "claude-haiku-4-5", activeEffort: "high" },
    );
    expect(reasoningButton().textContent).toContain("Default effort");
    reasoningButton().click();
    expect(reasoningItem("effort:high").className).not.toContain("on");
  });

  it("says on its button the model the agent is running on", () => {
    mount({ provider: "claude_adk", model: "", effort: "" }, { activeModel: "claude-opus-5" });
    expect(button().textContent).toContain("Claude Opus 5");
  });

  it("names the pending model beside the running one, and still checks the pending row", () => {
    mount({ provider: "claude_adk", model: "claude-haiku-4-5", effort: "" }, { activeModel: "claude-opus-5" });
    expect(button().textContent).toContain("Claude Opus 5 → Claude Haiku 4.5");
    expect(button().title).toBe("Running Claude Opus 5. Claude Haiku 4.5 at the next start.");
    button().click();
    expect(item("model:claude-haiku-4-5").className).toContain("on");
    expect(item("model:claude-opus-5").className).not.toContain("on");
  });

  it("shows the newly picked model as the next start's, the moment it is pressed", () => {
    mount({ provider: "claude_adk", model: "", effort: "" }, { activeModel: "claude-opus-5" });
    button().click();
    item("model:claude-haiku-4-5").click();
    expect(button().textContent).toContain("Claude Opus 5 → Claude Haiku 4.5");
  });

  it("leaves the open menu alone when nothing about the active model moved", () => {
    const { control } = mount(
      { provider: "claude_adk", model: "claude-opus-5", effort: "" },
      { activeModel: "claude-opus-5" },
    );
    button().click();
    const opened = menu();

    control.set(CATALOG, "claude_adk", { provider: "claude_adk", model: "claude-opus-5", effort: "" }, "claude-opus-5");

    expect(menu()).toBe(opened);
    expect(menu().hidden).toBe(false);
  });

  it("repaints when the model the agent is running on moves under it", () => {
    const { control } = mount({ provider: "claude_adk", model: "", effort: "" }, { activeModel: "claude-opus-5" });
    control.set(CATALOG, "claude_adk", { provider: "claude_adk", model: "", effort: "" }, "claude-haiku-4-5");
    expect(button().textContent).toContain("Claude Haiku 4.5");
  });
});

describe("the update note (#205)", () => {
  // An old CLI: the bridge hides the models it cannot run and says which.
  const OLD_CLI_CATALOG = {
    ...CATALOG,
    providers: [
      {
        ...CATALOG.providers[0],
        cli_name: "Claude Code",
        cli_version: "2.1.280",
        unavailable: [{ id: "claude-sonnet-5-5", label: "Claude Sonnet 5.5", requires_cli: "2.1.284" }],
      },
      CATALOG.providers[1],
    ],
  };
  const NOTE = "Update Claude Code to 2.1.284+ for Claude Sonnet 5.5.";
  const mountOld = (choice = { provider: "claude_adk", model: "claude-opus-5", effort: "high" }, options = {}) => {
    const mounted = mount(choice, options);
    mounted.control.set(OLD_CLI_CATALOG, choice.provider, choice, options.activeModel || "", options.activeEffort || "");
    return mounted;
  };
  const note = () => menu().querySelector(".menu-note");

  it("sits at the foot of the model list, muted, when the CLI is too old for some models", () => {
    mountOld();
    button().click();
    expect(note().textContent).toBe(NOTE);
    expect(note().classList.contains("model-update-note")).toBe(true);
    expect(menu().lastElementChild).toBe(note());
    expect(items()).toEqual(["model:claude-opus-5", "model:claude-haiku-4-5"]);
  });

  it("shows on an existing agent's menu too", () => {
    mountOld({ provider: "claude_adk", model: "", effort: "" }, { activeModel: "claude-opus-5", activeEffort: "high" });
    expect(note().textContent).toBe(NOTE);
  });

  it("is absent when the CLI runs every model, and from the reasoning menu", () => {
    mount({ provider: "claude_adk", model: "claude-opus-5", effort: "high" });
    expect(note()).toBeNull();
    mountOld();
    expect(reasoningMenu().querySelector(".menu-note")).toBeNull();
  });

  it("is text to a screen reader, never an option", () => {
    mountOld();
    expect(note().getAttribute("role")).toBe("none");
    expect(note().hasAttribute("tabindex")).toBe(false);
    expect(note().hasAttribute("data-action")).toBe(false);
    expect(menu().getAttribute("aria-describedby")).toBe(note().id);
    expect(note().id).not.toBe("");
  });

  it("does nothing when pressed and keeps the menu open", () => {
    const { onChoose } = mountOld();
    button().click();
    note().click();
    note().dispatchEvent(new Event("pointerdown", { bubbles: true }));
    expect(onChoose).not.toHaveBeenCalled();
    expect(menu().hidden).toBe(false);
  });

  it("never covers the row the keyboard lands on", () => {
    mountOld();
    const define = (element, values) => Object.entries(values).forEach(([key, value]) =>
      Object.defineProperty(element, key, { configurable: true, get: () => value }));
    [...menu().querySelectorAll(".mi")].forEach((row, index) =>
      define(row, { offsetTop: index * 40, offsetHeight: 40, offsetParent: menu() }));
    define(menu(), { clientHeight: 60 });
    define(note(), { offsetHeight: 30 });
    button().focus();
    button().dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true }));
    // The last row ends at 80; above a 30px note in a 60px menu, that is a
    // scroll of 50, not the 20 that would leave it under the note.
    expect(menu().scrollTop).toBe(50);
  });

  it("is skipped by the keyboard", () => {
    const { onChoose } = mountOld();
    button().focus();
    button().dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true }));
    expect(document.activeElement.dataset.action).toBe("model:claude-haiku-4-5");
    menu().dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    expect(document.activeElement.dataset.action).toBe("model:claude-opus-5");
    menu().dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true }));
    menu().dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    expect(onChoose).toHaveBeenCalledWith(expect.objectContaining({ model: "claude-haiku-4-5" }));
  });
});
