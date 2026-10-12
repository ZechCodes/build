import { expect, it } from "vitest";
import { writeFile } from "node:fs/promises";
import { withLayoutPage } from "./layoutHarness.mjs";
import { inspectProjectsFace, inspectActiveBlock, mountProjectsFace, openRecent, settleTheme, hoverRecentRow, useProductionStyles } from "./projectsFaceSeed.mjs";

// The project name's left edge, as #514 measured it: every child of a block
// stands on it (#516).
const CONTENT_X = { desktop: 51, phone: 64 };


/** Decode real screenshot pixels at the fold and project-name columns. */
async function blockColumnPixels(page) {
  const geometry = await page.locator(".inbox-project.active").evaluate((block) => {
    const bounds = block.getBoundingClientRect();
    return { clip: { x: Math.floor(bounds.x), y: Math.ceil(bounds.y), width: Math.ceil(bounds.width),
      height: Math.floor(bounds.bottom) - Math.ceil(bounds.y) },
      columns: [block.querySelector(".inbox-fold svg"), block.querySelector(".inbox-project-name")]
        .map((element) => Math.round(element.getBoundingClientRect().x) - Math.floor(bounds.x)) };
  });
  const png = await page.screenshot({ clip: geometry.clip, type: "png" });
  return page.evaluate(async ({ base64, columns }) => {
    const bitmap = await createImageBitmap(await (await fetch(`data:image/png;base64,${base64}`)).blob());
    const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
    const context = canvas.getContext("2d");
    context.drawImage(bitmap, 0, 0);
    const pixels = context.getImageData(0, 0, bitmap.width, bitmap.height).data;
    return columns.map((x) => Array.from({ length: bitmap.height }, (_, y) => {
      const offset = (y * bitmap.width + x) * 4;
      return `rgb(${pixels[offset]}, ${pixels[offset + 1]}, ${pixels[offset + 2]})`;
    }));
  }, { base64: png.toString("base64"), columns: geometry.columns });
}

/** Compare the real gutter pixel to the opaque mask's composite, allowing
 *  one RGB level for screenshot rounding of the translucent gradient. */
async function rowMaskPixelErrors(page, selector) {
  const row = page.locator(selector);
  const box = await row.boundingBox();
  const expected = await row.locator(".inbox-actions").evaluate((element) => getComputedStyle(element).backgroundColor);
  const png = await page.screenshot({ clip: { x: Math.floor(box.x + 5), y: Math.floor(box.y + box.height / 2), width: 1, height: 1 } });
  return page.evaluate(async ({ base64, expected }) => {
    const image = await createImageBitmap(await (await fetch(`data:image/png;base64,${base64}`)).blob());
    const canvas = new OffscreenCanvas(1, 1);
    const context = canvas.getContext("2d");
    context.drawImage(image, 0, 0);
    const actual = context.getImageData(0, 0, 1, 1).data;
    context.fillStyle = expected;
    context.fillRect(0, 0, 1, 1);
    const wanted = context.getImageData(0, 0, 1, 1).data;
    return [0, 1, 2].map((channel) => Math.abs(actual[channel] - wanted[channel]));
  }, { base64: png.toString("base64"), expected });
}

/** Encoded RGB brightness measures a visible step, independently of CSS formula. */
function brightness(rgb) {
  return rgb[0] * .2126 + rgb[1] * .7152 + rgb[2] * .0722;
}

async function gutterPixel(page, surface) {
  const box = await surface.boundingBox();
  const png = await page.screenshot({ clip: { x: Math.floor(box.x + 5), y: Math.floor(box.y + box.height / 2), width: 1, height: 1 } });
  return page.evaluate(async (base64) => {
    const bitmap = await createImageBitmap(await (await fetch(`data:image/png;base64,${base64}`)).blob());
    const canvas = new OffscreenCanvas(1, 1);
    const context = canvas.getContext("2d");
    context.drawImage(bitmap, 0, 0);
    return [...context.getImageData(0, 0, 1, 1).data].slice(0, 3);
  }, png.toString("base64"));
}

async function hoverPixels(page, selector, theme) {
  const surface = page.locator(selector);
  await surface.waitFor();
  await page.mouse.move(0, 0);
  await page.evaluate(() => document.activeElement?.blur());
  await settleTheme(page, theme);
  const rest = await gutterPixel(page, surface);
  const box = await surface.boundingBox();
  await page.mouse.move(box.x + 5, box.y + box.height / 2);
  await settleTheme(page, theme);
  expect(await surface.evaluate((element) => element.matches(":hover"))).toBe(true);
  return { rest, hover: await gutterPixel(page, surface) };
}

async function normalHoverStep(page) {
  const tones = await page.evaluate(() => {
    const context = new OffscreenCanvas(1, 1).getContext("2d");
    const probe = document.body.appendChild(document.createElement("span"));
    probe.style.transition = "none";
    const tones = ["--panel", "--row-hover"].map((token) => {
      probe.style.background = `var(${token})`;
      context.fillStyle = getComputedStyle(probe).backgroundColor;
      context.fillRect(0, 0, 1, 1);
      return [...context.getImageData(0, 0, 1, 1).data].slice(0, 3);
    });
    probe.remove();
    return tones;
  });
  return Math.abs(brightness(tones[0]) - brightness(tones[1]));
}

for (const { label, width, height, hasTouch } of [
  { label: "desktop", width: 1280, height: 800, hasTouch: false },
  { label: "phone", width: 390, height: 844, hasTouch: true },
]) {
  it(`lists quiet projects alphabetically with only the active block filled on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountProjectsFace(page, basePath);
      for (const theme of ["light", "dark"]) {
        await settleTheme(page, theme);
        const blocks = await inspectProjectsFace(page);
        expect(blocks.some((block) => block.active)).toBe(true);
        for (const block of blocks) {
          expect(block.background).toBe(block.active ? block.accentSoft : "rgba(0, 0, 0, 0)");
          expect(block.border).toBe("0px");
          expect(block.shadow).toBe("none");
        }
        expect(blocks.map((block) => block.name)).toEqual(["Alpha", "Bravo", "Build", "Zulu"]);
        expect(await page.locator("#inbox-list > .inbox-recent").count()).toBe(0);
        expect(await page.locator('[data-recent-toggle="projects-review/build"]').count()).toBe(1);
        expect(await page.locator('[data-project-fold][aria-expanded="true"]').count()).toBe(4);
        expect(await page.locator('.inbox-project .inbox-recent-toggle[aria-expanded="false"]').count()).toBe(2);
        expect(await page.locator('.inbox-project[data-project="projects-review/alpha"] .inbox-entry').count()).toBe(0);
        expect(await page.locator('[data-recent-toggle="projects-review/alpha"] .inbox-recent-count').textContent()).toBe("1");
      }
      await page.locator('[data-recent-toggle="projects-review/alpha"]').click();
      await page.locator('.inbox-recent [data-key="workspace:projects-review/alpha-quiet"]').waitFor();
      expect(await page.locator('.inbox-project[data-project="projects-review/alpha"] .inbox-quiet').count()).toBe(1);
    }, { width, height, hasTouch });
  }, 60_000);

  it(`indents each project's Recent into its content column on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountProjectsFace(page, basePath);
      await page.evaluate(() => window.__layoutModules.view.setInboxView("inbox"));
      const ungrouped = page.locator("#inbox-list > .inbox-recent > .inbox-recent-toggle");
      await ungrouped.waitFor();
      const ungroupedBox = await ungrouped.boundingBox();
      const ungroupedCaret = await ungrouped.locator(".inbox-recent-caret").boundingBox();
      expect(ungroupedBox.x).toBe(label === "phone" ? 33 : 20);
      expect(ungroupedCaret.x).toBe(ungroupedBox.x + 6);

      await page.evaluate(() => window.__layoutModules.view.setInboxView("projects"));
      const toggles = page.locator(".inbox-project .inbox-recent-toggle");
      expect(await toggles.count()).toBe(2);
      for (const expanded of [false, true]) {
        for (const theme of ["light", "dark"]) {
          await settleTheme(page, theme);
          for (const toggle of await toggles.all()) {
            const block = toggle.locator("..").locator("..");
            const caret = await toggle.locator(".inbox-recent-caret").boundingBox();
            const name = await block.locator(".inbox-project-name").boundingBox();
            const fold = await block.locator(".inbox-fold").boundingBox();
            expect(await toggle.getAttribute("aria-expanded")).toBe(String(expanded));
            expect(caret.x).toBeGreaterThanOrEqual(name.x);
            expect(caret.x).toBeGreaterThan(fold.x);
            if (expanded) {
              for (const quietTitle of await block.locator(".inbox-recent .stitle").all()) {
                expect(caret.x).toBe((await quietTitle.boundingBox()).x);
              }
            }
          }
        }
        if (!expanded) for (const toggle of await toggles.all()) await toggle.click();
      }
    }, { width, height, hasTouch });
  }, 60_000);

  it(`stands a block's rows and Recent on one content column on ${label} (#516)`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountProjectsFace(page, basePath);
      await openRecent(page, "projects-review/build");
      const block = await inspectActiveBlock(page);
      expect(block.name.x).toBe(CONTENT_X[label]);
      expect(block.caret.x).toBe(CONTENT_X[label]);
      expect(block.rows.map((row) => row.key)).toEqual(["workspace:projects-review/build-live",
        "tracker_task:land-tracker", "tracker_task:release-check", "workspace:projects-review/build-quiet"]);
      for (const row of block.rows) expect(row.title.x, row.key).toBe(CONTENT_X[label]);
    }, { width, height, hasTouch });
  }, 60_000);

  it(`keeps a quiet row's name readable beside its facts on ${label} (#516)`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountProjectsFace(page, basePath);
      await openRecent(page, "projects-review/build");
      const quiet = (await inspectActiveBlock(page)).rows.find((row) => row.facts);
      expect(quiet.text).toBe("Older workspace");
      expect(quiet.clipped).toBe(false);
      expect(quiet.facts.x).toBeGreaterThanOrEqual(quiet.title.right);
      expect(quiet.facts.x).toBeGreaterThan(quiet.row.x);
      expect(quiet.at).toBe(true);
    }, { width, height, hasTouch });
  }, 60_000);

  it(`keeps every right-edge control on one column on ${label} (#516)`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountProjectsFace(page, basePath);
      await openRecent(page, "projects-review/build");
      const block = await inspectActiveBlock(page);
      const column = block.create.right;
      const [live, ...quiet] = block.rows;
      expect(live.done.right).toBe(column);
      for (const row of quiet) {
        if (row.watch) expect(row.watch.right, row.key).toBe(column);
        // The eye's glyph stands where the head's + glyph does, whatever its touch target.
        if (row.watch) expect(Math.abs(row.watchGlyph.right - block.createGlyph.right), row.key).toBeLessThanOrEqual(1);
        if (row.facts) expect(row.facts.right, row.key).toBe(column);
      }
      expect(block.count.right).toBe(column);
      expect(quiet.filter((row) => row.watch)).toHaveLength(2);
    }, { width, height, hasTouch });
  }, 60_000);

  it(`lets the active block own its head, Recent and resting rows on ${label} (#516)`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await page.emulateMedia({ reducedMotion: "reduce" });
      await mountProjectsFace(page, basePath);
      await openRecent(page, "projects-review/build");
      for (const theme of ["light", "dark"]) {
        await page.evaluate((theme) => {
          document.documentElement.dataset.theme = theme;
          for (const animation of document.getAnimations()) if (animation instanceof CSSTransition) animation.finish();
        }, theme);
        const block = await inspectActiveBlock(page);
        expect(block.background, theme).toBe(block.accentSoft);
        expect(block.headFill, theme).toBe("rgba(0, 0, 0, 0)");
        expect(block.toggleFill, theme).toBe("rgba(0, 0, 0, 0)");
        expect(block.toggle.x, theme).toBe(block.head.x);
        expect(block.toggle.right, theme).toBe(block.head.right);
        // Resting rows inherit the block fill for their masks and paint no fill themselves.
        for (const row of block.rows) {
          expect(row.inheritedFill, `${theme} ${row.key}`).toBe(block.accentSoft);
          expect(row.actionsFill, `${theme} ${row.key}`).toBe(block.accentSoft);
          expect(row.fill, `${theme} ${row.key}`).toBe("rgba(0, 0, 0, 0)");
        }
        const inactive = await page.locator('[data-recent-toggle="projects-review/alpha"]')
          .evaluate((toggle) => getComputedStyle(toggle).backgroundColor);
        expect(inactive, theme).toBe("rgba(0, 0, 0, 0)");
      }
    }, { width, height, hasTouch });
  }, 60_000);

  for (const theme of ["light", "dark"]) {
    for (const emptyLive of [false, true]) {
      it(`has no rail or panel seam through the active and folded block on ${label} ${theme}, empty live=${emptyLive} (#540)`, async () => {
        await withLayoutPage(async ({ page, basePath }) => {
          await page.emulateMedia({ reducedMotion: "reduce" });
          await mountProjectsFace(page, basePath, { standing: true, emptyLive });
          await useProductionStyles(page);
          await openRecent(page, "projects-review/build");
          await settleTheme(page, theme);
          await hoverRecentRow(page);
          expect(await page.locator(".inbox-project.active .inbox-recent .inbox-entry.active").count()).toBe(1);
          const block = await inspectActiveBlock(page);
          const standing = await page.locator(".inbox-project.active .inbox-entry.active").evaluate((element) => ({
            background: getComputedStyle(element).backgroundColor,
            bar: getComputedStyle(element, "::before").width,
          }));
          expect(standing.background).not.toBe(block.accentSoft);
          expect(standing.bar).toBe("3px");
          const columns = await blockColumnPixels(page);
          for (const [column, pixels] of columns.entries()) {
            expect(pixels.filter((pixel) => pixel === block.panel || pixel === block.ground), `column ${column}`).toEqual([]);
          }
          if (emptyLive) {
            const rows = await page.locator(".inbox-project.active .inbox-project-rows").boundingBox();
            expect(rows.height).toBe(0);
          }
          const expanded = await page.locator(".inbox-project.active").boundingBox();
          await page.locator('[data-project-fold="projects-review/build"]').click();
          await page.locator(".inbox-project.active.inbox-folded").waitFor();
          await page.mouse.move(0, 0);
          await page.evaluate(() => document.activeElement?.blur());
          await settleTheme(page, theme);
          expect(await page.locator(".inbox-project.active .inbox-recent").isVisible()).toBe(false);
          expect((await page.locator(".inbox-project.active").boundingBox()).height).toBeLessThan(expanded.height);
          for (const [column, pixels] of (await blockColumnPixels(page)).entries()) {
            expect(pixels.filter((pixel) => pixel === block.panel || pixel === block.ground), `folded column ${column}`).toEqual([]);
            expect(pixels[0], `folded column ${column} top`).toBe(block.accentSoft);
            expect(pixels.at(-1), `folded column ${column} bottom`).toBe(block.accentSoft);
          }
        }, { width, height, hasTouch });
      }, 60_000);
    }
  }

  for (const theme of ["light", "dark"]) {
    it(`visibly darkens standing, resting, ungrouped and head hover on ${label} ${theme} (#540)`, async () => {
      await withLayoutPage(async ({ page, basePath }) => {
        await page.emulateMedia({ reducedMotion: "reduce" });
        await mountProjectsFace(page, basePath, { standing: true });
        await useProductionStyles(page);
        await openRecent(page, "projects-review/build");
        await settleTheme(page, theme);
        const referenceStep = await normalHoverStep(page);
        expect(referenceStep).toBeGreaterThan(10);
        const minimumStep = referenceStep / 2;
        const samples = [];
        const surfaces = [
          ["standing", ".inbox-project.active .inbox-entry.active"],
          ["resting", '.inbox-project.active [data-key="workspace:projects-review/build-live"]'],
          ["head", ".inbox-project.active .inbox-project-head"],
          ["inactive head", '.inbox-project[data-project="projects-review/alpha"] .inbox-project-head'],
        ];
        for (const [label, selector] of surfaces) {
          const { rest, hover } = await hoverPixels(page, selector, theme);
          samples.push({ surface: label, rest, hover, step: brightness(rest) - brightness(hover) });
          for (const [channel, value] of hover.entries()) expect.soft(value, `${label} RGB ${channel}`).toBeLessThan(rest[channel]);
          expect.soft(brightness(rest) - brightness(hover), `${label} visible step`).toBeGreaterThanOrEqual(minimumStep);
          if (selector.includes("inbox-entry")) {
            for (const error of await rowMaskPixelErrors(page, selector)) expect.soft(error, `${label} mask`).toBeLessThanOrEqual(1);
          }
        }
        await page.evaluate(() => window.__layoutModules.view.setInboxView("inbox"));
        const selector = '#inbox-list > [data-key="workspace:projects-review/build-live"]';
        const { rest, hover } = await hoverPixels(page, selector, theme);
        samples.push({ surface: "ungrouped", rest, hover, step: brightness(rest) - brightness(hover) });
        for (const [channel, value] of hover.entries()) expect.soft(value, `ungrouped RGB ${channel}`).toBeLessThan(rest[channel]);
        expect.soft(brightness(rest) - brightness(hover), "ungrouped visible step").toBeGreaterThanOrEqual(minimumStep);
        for (const error of await rowMaskPixelErrors(page, selector)) expect.soft(error, "ungrouped mask").toBeLessThanOrEqual(1);
        if (process.env.BUILD_HOVER_EVIDENCE) await writeFile(`${process.env.BUILD_HOVER_EVIDENCE}/${label}-${theme}.json`,
          `${JSON.stringify({ width, theme, referenceStep, minimumStep, samples }, null, 2)}\n`);
      }, { width, height, hasTouch });
    }, 60_000);
  }

  it(`keeps all block insets in padding and a folded block filled on ${label} (#540)`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountProjectsFace(page, basePath);
      await openRecent(page, "projects-review/build");
      const margins = await page.locator(".inbox-project.active")
        .locator(".inbox-project-head, .inbox-recent-toggle, .inbox-entry").evaluateAll((elements) =>
          elements.map((element) => {
            const style = getComputedStyle(element);
            return [style.marginLeft, style.marginRight, style.marginTop, style.marginBottom];
          }));
      for (const margin of margins) expect(margin).toEqual(["0px", "0px", "0px", "0px"]);
      await page.locator('[data-project-fold="projects-review/build"]').click();
      await page.locator(".inbox-project.active.inbox-folded").waitFor();
      expect(await page.locator(".inbox-project.active .inbox-recent").isVisible()).toBe(false);
      const folded = await inspectActiveBlock(page);
      expect(folded.background).toBe(folded.accentSoft);
      expect(folded.headFill).toBe("rgba(0, 0, 0, 0)");
    }, { width, height, hasTouch });
  }, 60_000);

  it(`keeps standing and hover fills under the production stylesheet order on ${label} (#540)`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await page.emulateMedia({ reducedMotion: "reduce" });
      await mountProjectsFace(page, basePath, { standing: true });
      await openRecent(page, "projects-review/build");
      // The app's styles.css imports shell before defining the shared .srow
      // rules. Drop the harness's duplicate shell sheets to exercise that order.
      await useProductionStyles(page);
      for (const theme of ["light", "dark"]) {
        await settleTheme(page, theme);
        const block = await inspectActiveBlock(page);
        const standing = block.rows.find((row) => row.key === "tracker_task:land-tracker");
        expect(standing.fill).not.toBe(block.accentSoft);
        expect(standing.fill).toBe(standing.actionsFill);
        await hoverRecentRow(page);
        const hover = await page.locator('.inbox-project.active .inbox-entry:hover')
          .evaluate((element) => getComputedStyle(element).backgroundImage);
        expect(hover).toContain("linear-gradient");
        for (const selector of [".inbox-project.active .inbox-entry.active", ".inbox-project.active .inbox-entry:hover"]) {
          const errors = await rowMaskPixelErrors(page, selector);
          for (const error of errors) expect(error, `${theme} ${selector}`).toBeLessThanOrEqual(1);
        }
      }
    }, { width, height, hasTouch });
  }, 60_000);

  it(`keeps the full-width Recent heading's focus ring inside the rail on ${label} (#516)`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountProjectsFace(page, basePath);
      await openRecent(page, "projects-review/build");
      const toggle = page.locator('[data-recent-toggle="projects-review/build"]');
      await toggle.focus();
      await page.keyboard.press("Tab");
      await page.keyboard.press("Shift+Tab");
      const ring = await toggle.evaluate((element) => {
        const style = getComputedStyle(element);
        const grow = parseFloat(style.outlineOffset) + parseFloat(style.outlineWidth);
        const box = element.getBoundingClientRect();
        const list = document.querySelector("#inbox-list").getBoundingClientRect();
        return { focused: element.matches(":focus-visible"), drawn: style.outlineStyle !== "none" && grow > -box.width / 2,
          left: box.left - grow, right: box.right + grow, top: box.top - grow, bottom: box.bottom + grow,
          width: box.width, list: { left: list.left, right: list.right, top: list.top, bottom: list.bottom } };
      });
      expect(ring.focused).toBe(true);
      expect(ring.drawn).toBe(true);
      expect(ring.left).toBeGreaterThanOrEqual(ring.list.left);
      expect(ring.right).toBeLessThanOrEqual(ring.list.right);
      expect(ring.top).toBeGreaterThanOrEqual(ring.list.top);
      expect(ring.bottom).toBeLessThanOrEqual(ring.list.bottom);
      // The fill still spans the rail.
      expect(ring.width).toBe(ring.list.right - ring.list.left);
    }, { width, height, hasTouch });
  }, 60_000);
}

it("includes the phone safe area in the block's single inset (#540)", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    const session = await page.context().newCDPSession(page);
    await session.send("Emulation.setSafeAreaInsetsOverride", { insets: { left: 20 } });
    await mountProjectsFace(page, basePath);
    await openRecent(page, "projects-review/build");
    const block = await inspectActiveBlock(page);
    const list = await page.locator("#inbox-list").boundingBox();
    // The safe area shifts the floating rail as well as its content inset.
    expect(block.name.x - list.x).toBe(41 + 20);
    expect(block.caret.x).toBe(block.name.x);
    for (const row of block.rows) expect(row.title.x).toBe(block.name.x);
    expect(block.head.x).toBe(list.x);
    expect(block.head.right).toBe(list.x + list.width);
  }, { width: 390, height: 844, hasTouch: true });
}, 60_000);
