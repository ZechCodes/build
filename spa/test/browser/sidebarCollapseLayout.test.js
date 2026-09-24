import { expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

// The checkout's list column (the file tree, the commit rail) folds away from
// the toggle at the foot of the directory rail, in a real browser against the
// production sheets: the detail takes the whole width, nothing else moves, and
// where the list column is a drawer the toggle is gone and the drawer works
// whatever the stored choice says.

const rows = Array.from({ length: 14 }, (_, index) => `<div class="pick">file-${index}.md</div>`).join("");
const markup = `<div id="shell"><aside id="inbox-rail"></aside><div id="view">
  <header id="toolbar">Build / Workspace</header>
  <div id="view-body"><nav id="dir-rail" aria-label="Directory views"></nav>
    <main id="root" class="surface"><div id="tabbody" class="flush">
      <div class="files pane-split"><div class="ftree pane-list" id="ftree"><div class="ftree-list">${rows}</div></div>
      <div class="fpreview" id="fpreview"><h1 style="padding:0 16px">AGENTS.md</h1></div></div>
    </div></main>
    <aside id="agent-rail" aria-label="Agents"><div style="width:280px">Conversation</div></aside>
  </div><div id="console-region"></div>
</div></div>`;

// No inbox rail: below 900px it is an overlay, and nothing here opens or shuts it.
const STYLES = "#inbox-rail{display:none} #view{height:calc(100vh - 20px)}";

async function mountCheckout(page, basePath, { collapsed = false } = {}) {
  await mountLayout(page, markup, { basePath, styles: STYLES });
  await loadBrowserModules(page, { rail: "src/core/directoryRail.js", drawer: "src/core/paneDrawer.js" }, basePath);
  await page.evaluate((stored) => {
    document.documentElement.dataset.theme = "dark";
    // The pane is laid out before the rail paints — the order a surface that
    // measures its own pane on mount paints in.
    void document.body.offsetWidth;
    localStorage.setItem(window.__layoutModules.rail.SIDEBAR_COLLAPSED_KEY, String(stored));
    const split = document.querySelector(".pane-split");
    split.insertAdjacentHTML("beforeend", window.__layoutModules.drawer.paneDrawerHtml("files"));
    window.__drawer = window.__layoutModules.drawer.initPaneDrawer(split, {
      list: document.querySelector("#ftree"), closeOnSelect: ".pick",
    });
    window.__layoutModules.rail.paintDirectoryRail(document.querySelector("#dir-rail"), {
      active: "files", onSelect: () => {},
    });
  }, collapsed);
}

const geometry = (page) => page.evaluate(() => {
  const box = (selector) => {
    const { x, y, width, height, right, bottom } = document.querySelector(selector).getBoundingClientRect();
    return { x, y, width, height, right, bottom };
  };
  const toggle = document.querySelector("[data-sidebar-toggle]");
  return {
    rail: box("#dir-rail"), list: box("#ftree"), preview: box("#fpreview"), split: box(".pane-split"),
    toolbar: box("#toolbar"), agentRail: box("#agent-rail"),
    toggle: toggle?.getClientRects().length ? box("[data-sidebar-toggle]") : null,
    listVisibility: getComputedStyle(document.querySelector("#ftree")).visibility,
  };
});

/** Wait out the width transition: the list column stops changing size. */
const settle = (page) => page.evaluate(() => Promise.all(
  document.querySelector("#ftree").getAnimations().map((animation) => animation.finished.catch(() => {})),
));

it("folds the list column away from the rail's foot and brings it back at its width", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountCheckout(page, basePath);
    const expanded = await geometry(page);
    expect(expanded.list.width).toBeGreaterThan(200);
    // The toggle stands at the very bottom of the rail, in the tab cells' idiom.
    expect(expanded.toggle.width).toBe(32);
    expect(expanded.toggle.height).toBe(32);
    expect(expanded.rail.bottom - expanded.toggle.bottom).toBeLessThanOrEqual(8);
    await captureLayout(page, "sidebar-expanded.png");

    await page.click("[data-sidebar-toggle]");
    // The width moves, briefly: the list is still standing mid-transition.
    expect(await page.evaluate(() => document.querySelector("#ftree").getAnimations().length)).toBeGreaterThan(0);
    await settle(page);
    const collapsed = await geometry(page);
    expect(collapsed.list.width).toBe(0);
    expect(collapsed.listVisibility).toBe("hidden");
    // The detail takes the whole width, from the rail's edge to the split's.
    expect(collapsed.preview.x).toBeCloseTo(collapsed.rail.right, 0);
    expect(collapsed.preview.right).toBeCloseTo(expanded.preview.right, 0);
    // Nothing else on the surface moves.
    expect(collapsed.toolbar).toEqual(expanded.toolbar);
    expect(collapsed.agentRail).toEqual(expanded.agentRail);
    expect(collapsed.rail).toEqual(expanded.rail);
    expect(collapsed.split).toEqual(expanded.split);
    expect(collapsed.toggle).toEqual(expanded.toggle);
    await captureLayout(page, "sidebar-collapsed.png");

    await page.click("[data-sidebar-toggle]");
    await settle(page);
    expect(await geometry(page)).toEqual(expanded);
  }, { width: 1280, height: 820 });
}, 30_000);

it("moves nothing under reduced motion", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await mountCheckout(page, basePath);
    await page.click("[data-sidebar-toggle]");
    expect(await page.evaluate(() => document.querySelector("#ftree").getAnimations().length)).toBe(0);
    expect((await geometry(page)).list.width).toBe(0);
  }, { width: 1280, height: 820 });
}, 30_000);

it("paints a stored collapse from the first frame, and a repaint moves nothing", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountCheckout(page, basePath, { collapsed: true });
    // Only a press moves the column: arriving on a checkout that was left
    // folded is not a fold.
    expect(await page.evaluate(() => document.querySelector("#ftree").getAnimations().length)).toBe(0);
    expect((await geometry(page)).list.width).toBe(0);
    await page.click("[data-sidebar-toggle]");
    await page.evaluate(() => window.__layoutModules.rail.paintDirectoryRail(document.querySelector("#dir-rail"), {
      active: "changes", onSelect: () => {},
    }));
    await settle(page);
    await page.evaluate(() => {
      document.querySelector("#dir-rail").innerHTML = "";
      void document.body.offsetWidth;
      localStorage.setItem(window.__layoutModules.rail.SIDEBAR_COLLAPSED_KEY, "true");
      window.__layoutModules.rail.paintDirectoryRail(document.querySelector("#dir-rail"), {
        active: "files", onSelect: () => {},
      });
    });
    expect(await page.evaluate(() => document.querySelector("#ftree").getAnimations().length)).toBe(0);
  }, { width: 1280, height: 820 });
}, 30_000);

it("leaves a pane-split off a checkout alone, whatever the rail remembers", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountCheckout(page, basePath, { collapsed: true });
    // Leaving a checkout empties the rail (workspaceView, branchView); the
    // attribute it wore is still on it.
    await page.evaluate(() => { document.querySelector("#dir-rail").innerHTML = ""; });
    expect((await geometry(page)).list.width).toBeGreaterThan(200);
  }, { width: 1280, height: 820 });
}, 30_000);

for (const width of [900, 800, 390]) {
  it(`hides the toggle and ignores the stored collapse where the list is a drawer (${width}px)`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountCheckout(page, basePath, { collapsed: true });
      const shut = await geometry(page);
      expect(shut.toggle).toBeNull();
      await page.click("[data-pane-handle]");
      await settle(page);
      const open = await geometry(page);
      await captureLayout(page, `sidebar-drawer-${width}.png`);
      expect(open.listVisibility).toBe("visible");
      expect(open.list.width).toBeCloseTo(open.split.width, 0);
      expect(open.list.height).toBeGreaterThan(100);
      expect(open.list.y).toBeGreaterThanOrEqual(open.split.y);
    }, { width, height: 820 });
  }, 30_000);
}
