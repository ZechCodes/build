import { expect, it } from "vitest";
import { withLayoutPage } from "./layoutHarness.mjs";
import { mountStatusDotInbox } from "./inboxStatusDotSeed.mjs";

const rowKeys = ["workspace:layout-device/attention", "task:layout-device/layout-project/380", "branch:quiet"];
const contexts = [
  { label: "active project", grouped: true, activeProject: true },
  { label: "inactive project", grouped: true, activeProject: false },
  { label: "ungrouped inbox", grouped: false },
  { label: "project page", grouped: false, projectPage: true },
];
// Start with the reported regression; cover every hover/selection/focus combination.
const states = [
  { label: "hovered", hover: true },
  { label: "resting" },
  { label: "active", active: true },
  { label: "active hovered", active: true, hover: true },
  { label: "focused", focus: true },
  { label: "focused hovered", focus: true, hover: true },
  { label: "active focused", active: true, focus: true },
  { label: "active focused hovered", active: true, focus: true, hover: true },
  { label: "menu open", menu: true },
];

async function rowFill(row, expectedFill, state) {
  return row.evaluate(async (element, { expectedFill, state }) => {
    // This checks state colours, not motion. Reduced motion still leaves
    // .01ms transitions on the row and overlays that can start after frame callbacks.
    document.activeElement?.blur();
    element.classList.toggle("active", Boolean(state.active));
    const menu = element.querySelector(".inbox-menu");
    if (menu) menu.hidden = !state.menu;
    if (state.focus) element.querySelector(".inbox-actions > button").focus();
    element.getBoundingClientRect();
    const transitions = element.getAnimations({ subtree: true })
      .filter((animation) => animation instanceof CSSTransition);
    await Promise.all(transitions.map((transition) => {
      transition.finish();
      return transition.finished;
    }));
    const probe = document.createElement("span");
    probe.style.background = expectedFill;
    element.append(probe);
    const expected = getComputedStyle(probe).backgroundColor;
    probe.remove();
    return {
      connected: element.isConnected,
      active: element.classList.contains("active"),
      hovered: element.matches(":hover"),
      background: getComputedStyle(element).backgroundColor,
      paint: getComputedStyle(element).backgroundImage,
      transparentRest: Boolean(element.closest(".inbox-project.active") && !state.active),
      expected,
      overlays: [...element.querySelectorAll(".inbox-actions, .inbox-facts-float")].map((overlay) => ({
        className: overlay.className,
        background: getComputedStyle(overlay).backgroundColor,
        shadow: getComputedStyle(overlay).boxShadow,
        // The facts stand beside the name now, not over it, so only the actions fade (#516).
        fade: overlay.classList.contains("inbox-actions") ? "-8px 0px 8px -6px" : null,
      })),
      focused: element.matches(":focus-within"),
    };
  }, { expectedFill, state });
}

function expectedFill(context, state, theme) {
  if (context.projectPage && state.hover) return "var(--line2)";
  const inActiveBlock = context.grouped && context.activeProject;
  const standing = inActiveBlock ? "color-mix(in srgb, var(--accent-soft) 90%, var(--accent-ink))" : "var(--accent-soft)";
  const base = state.active ? standing : inActiveBlock ? "var(--accent-soft)" : "var(--panel)";
  const ink = theme === "light" ? "var(--ink)" : "rgb(from var(--accent-ink) calc(r * .1) calc(g * .1) calc(b * .1))";
  const weight = theme === "light" ? "12%" : "70%";
  return state.hover ? `color-mix(in srgb, ${ink} ${weight}, ${base})` : base;
}

it("samples final row and overlay fills while their colour transitions are pending", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await page.emulateMedia({ reducedMotion: "no-preference" });
    await mountStatusDotInbox(page, basePath);
    const row = await page.locator(`.inbox-entry[data-key="${rowKeys[0]}"]`).elementHandle();
    const pending = await row.evaluate((element) => {
      // Keep real transitions pending long enough to reproduce the stale samples.
      const overlay = element.querySelector(".inbox-actions");
      element.style.transitionDuration = "60s";
      overlay.style.transitionDuration = "60s";
      element.getBoundingClientRect();
      element.classList.add("active");
      element.getBoundingClientRect();
      const animations = element.getAnimations({ subtree: true });
      return [element, overlay].map((target) => animations.some((animation) =>
        animation instanceof CSSTransition && animation.effect.target === target && animation.playState === "running"));
    });
    expect(pending[0]).toBe(true);
    expect(pending[1]).toBe(true);
    const fill = await rowFill(row, "var(--accent-soft)", { active: true });
    expect(fill.connected).toBe(true);
    expect(fill.active).toBe(true);
    expect(fill.background).toBe(fill.expected);
    expect(fill.overlays[0].background).toBe(fill.expected);
    expect(fill.overlays[0].shadow).toBe(`${fill.expected} ${fill.overlays[0].fade}`);
    await row.dispose();
  });
});

for (const theme of ["light", "dark"]) {
  for (const touch of [false, true]) {
    it(`matches inbox overlay fills to their rows in ${theme} with ${touch ? "touch" : "a pointer"}`, async () => {
      await withLayoutPage(async ({ page, basePath }) => {
        await page.emulateMedia({ reducedMotion: "reduce" });
        const session = await page.context().newCDPSession(page);
        await session.send("DOM.enable");
        await session.send("CSS.enable");
        if (touch) await session.send("Emulation.setTouchEmulationEnabled", { enabled: true });
        expect(await page.evaluate(() => matchMedia("(hover:none)").matches)).toBe(touch);
        for (const context of contexts) {
          await mountStatusDotInbox(page, basePath, { grouped: context.grouped });
          await page.evaluate(({ theme, context }) => {
            document.documentElement.dataset.theme = theme;
            document.querySelector(".inbox-project")?.classList.toggle("active", Boolean(context.activeProject));
            document.querySelector("#inbox-list").classList.toggle("project-page", Boolean(context.projectPage));
          }, { theme, context });
          const { root } = await session.send("DOM.getDocument");
          for (const key of rowKeys) {
            const selector = `.inbox-entry[data-key="${key}"]`;
            const row = await page.locator(selector).elementHandle();
            await row.evaluate((element) => {
              const entry = window.__statusDotEntries.find((entry) => entry.key === element.dataset.key);
              const template = document.createElement("template");
              template.innerHTML = window.__layoutModules.rows.inboxRowHtml(entry, {
                quiet: element.classList.contains("inbox-quiet"), openMenuKey: entry.key,
              });
              const menu = template.content.querySelector(".inbox-menu");
              if (menu) {
                menu.hidden = true;
                element.querySelector(".inbox-actions").append(menu);
              }
            });
            const { nodeId } = await session.send("DOM.querySelector", { nodeId: root.nodeId, selector });
            for (const state of states) {
              await session.send("CSS.forcePseudoState", { nodeId, forcedPseudoClasses: state.hover ? ["hover"] : [] });
              const label = `${context.label}: ${key}, ${state.label}`;
              const fill = await rowFill(row, expectedFill(context, state, theme), state);
              expect(fill.connected, label).toBe(true);
              expect(fill.active, label).toBe(Boolean(state.active));
              expect(fill.hovered, label).toBe(Boolean(state.hover));
              expect(fill.focused, label).toBe(Boolean(state.focus));
              // Grouped quiet rows keep floating facts beside their names;
              // Inbox quiet rows put activity below the name instead.
              const floatingFacts = key === "branch:quiet" && context.grouped;
              expect(fill.overlays.length, label).toBe(floatingFacts ? 2 : 1);
              for (const overlay of fill.overlays) {
                expect(overlay.background, `${label}: ${overlay.className}`).toBe(fill.expected);
                expect(overlay.shadow, `${label}: ${overlay.className} fade`)
                  .toBe(overlay.fade ? `${fill.expected} ${overlay.fade}` : "none");
              }
              if (fill.transparentRest && !context.projectPage) expect(fill.background, label).toBe("rgba(0, 0, 0, 0)");
              if (state.hover && !context.projectPage) expect(fill.paint, label).toContain("linear-gradient");
              if (!state.hover && !fill.transparentRest) expect(fill.background, label).toBe(fill.expected);
            }
            await session.send("CSS.forcePseudoState", { nodeId, forcedPseudoClasses: [] });
            await row.dispose();
          }
        }
      });
    }, 60_000);
  }
}
