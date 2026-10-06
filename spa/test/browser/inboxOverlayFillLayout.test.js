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

async function rowFill(row, token) {
  return row.evaluate((element, token) => {
    const probe = document.createElement("span");
    probe.style.background = `var(${token})`;
    element.append(probe);
    const expected = getComputedStyle(probe).backgroundColor;
    probe.remove();
    return {
      background: getComputedStyle(element).backgroundColor,
      expected,
      overlays: [...element.querySelectorAll(".inbox-actions, .inbox-facts-float")].map((overlay) => ({
        className: overlay.className,
        background: getComputedStyle(overlay).backgroundColor,
        shadow: getComputedStyle(overlay).boxShadow,
        fade: overlay.classList.contains("inbox-actions") ? "-8px 0px 8px -6px" : "-10px 0px 10px -6px",
      })),
      focused: element.matches(":focus-within"),
    };
  }, token);
}

function expectedToken(context, state) {
  if (context.projectPage && state.hover) return "--line2";
  if (state.active) return "--accent-soft";
  if (state.hover) return "--row-hover";
  if (context.activeProject) return "--accent-soft";
  return context.grouped ? "--panel2" : "--panel";
}

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
            const row = page.locator(selector);
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
              await row.evaluate((element, state) => {
                document.activeElement?.blur();
                element.classList.toggle("active", Boolean(state.active));
                const menu = element.querySelector(".inbox-menu");
                if (menu) menu.hidden = !state.menu;
                if (state.focus) element.querySelector(".inbox-actions > button").focus();
              }, state);
              // Read settled state colours, after the shared row transition.
              await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
              const label = `${context.label}: ${key}, ${state.label}`;
              const fill = await rowFill(row, expectedToken(context, state));
              expect(fill.focused, label).toBe(Boolean(state.focus));
              expect(fill.overlays.length, label).toBe(key === "branch:quiet" ? 2 : 1);
              for (const overlay of fill.overlays) {
                expect(overlay.background, `${label}: ${overlay.className}`).toBe(fill.background);
                expect(overlay.shadow, `${label}: ${overlay.className} fade`).toBe(`${fill.background} ${overlay.fade}`);
              }
              expect(fill.background, `${label}: row colour`).toBe(fill.expected);
            }
            await session.send("CSS.forcePseudoState", { nodeId, forcedPseudoClasses: [] });
          }
        }
      });
    }, 60_000);
  }
}
