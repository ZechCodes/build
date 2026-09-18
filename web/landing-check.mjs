// Exercise the public story in a browser. Start scripts/preview-landing.py first.
// LANDING_URL and LANDING_REVIEW_DIR allow a deployed preview and artifact folder.
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { chromium } from "playwright";

const base = process.env.LANDING_URL || "http://127.0.0.1:4173";
const output = process.env.LANDING_REVIEW_DIR || "/tmp/build-landing-review";
await fs.mkdir(output, { recursive: true });
const browser = await chromium.launch({
  headless: process.env.LANDING_HEADFUL !== "1",
  executablePath: process.env.CHROMIUM_PATH,
  args: process.env.LANDING_HEADFUL === "1" ? ["--ozone-platform=x11"] : ["--enable-unsafe-swiftshader"],
});
const findings = [];
const sizes = [[1440, 900], [1920, 1080], [768, 1024], [1024, 768], [390, 844], [360, 740], [844, 390]];

async function settle(page) {
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
}

async function layout(page) {
  return page.evaluate(() => ({
    width: innerWidth,
    documentWidth: document.documentElement.scrollWidth,
    headings: [...document.querySelectorAll("[data-story-scene] h1, [data-story-scene] h2")].map((element) => element.textContent.trim()),
    state: window.BuildLandingStory?.getState?.(),
    ready: document.querySelector("[data-device-stage]")?.dataset.webglReady,
    missingImages: [...document.images].filter((image) => image.complete && !image.naturalWidth).map((image) => image.src),
  }));
}

async function inspectViewport(width, height) {
  const context = await browser.newContext({ viewport: { width, height } });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(base, { waitUntil: "networkidle" });
  await settle(page);
  const initial = await layout(page);
  assert.equal(initial.headings.length, 6, "all six semantic claims must exist");
  assert(initial.documentWidth <= width + 1, `horizontal overflow at ${width}×${height}`);
  assert.deepEqual(initial.missingImages, [], "all visible media should resolve");
  if (height >= 600) assert(initial.state?.enhanced || initial.state?.staticReason, "normal viewports enhance or report a static fallback reason");
  await page.screenshot({ path: path.join(output, `${width}x${height}-start.png`), animations: "disabled" });
  if (initial.state?.enhanced) {
    await inspectChapters(page, `${width}x${height}`);
  }
  await page.locator("#download").scrollIntoViewIfNeeded();
  await settle(page);
  assert(await page.locator("#download").isVisible(), "installer chooser is reachable");
  assert(await page.locator('#download a[href="/app/"]').count(), "chooser uses the existing alpha setup flow");
  await page.screenshot({ path: path.join(output, `${width}x${height}-download.png`) });
  assert.deepEqual(errors, [], `browser errors at ${width}×${height}`);
  findings.push({ viewport: [width, height], ...initial, browserErrors: errors });
  await context.close();
}

async function seek(page, sceneIndex, local) {
  await page.evaluate(async ({ sceneIndex, local }) => {
    const { travelAtFrame } = await import("/landing/story-manifest.js");
    const story = document.querySelector("[data-story]");
    const stage = document.querySelector("[data-story-stage]");
    const state = window.BuildLandingStory.getState();
    const origin = story.getBoundingClientRect().top + scrollY;
    scrollTo({ top: origin + travelAtFrame(sceneIndex, local, state.profile) * stage.getBoundingClientRect().height, behavior: "instant" });
  }, { sceneIndex, local });
  await settle(page);
}

function assertFitsViewport(box, viewport, label) {
  assert(box && box.x >= 0 && box.y >= 0 && box.x + box.width <= viewport.width + 1 && box.y + box.height <= viewport.height, `${label} fits the viewport`);
}

async function inspectActivity(page, headingBox) {
  const activity = await page.locator(".demo--activity").boundingBox();
  const viewport = page.viewportSize();
  assertFitsViewport(activity, viewport, "the entire activity card");
  if (viewport.width < 768) assert(activity.y >= headingBox.y + headingBox.height, "the mobile activity card clears its claim");
}

async function inspectChapters(page, label) {
  for (const index of [1, 2, 3, 4, 5, 3, 1, 0]) {
    await seek(page, index, 0.5);
    const current = await layout(page);
    assert.equal(current.state.sceneIndex, index, "forward/reverse seeks restore the exact scene");
    const heading = page.locator("[data-story-scene]").nth(index).locator("h1,h2");
    const box = await heading.boundingBox();
    assertFitsViewport(box, page.viewportSize(), "the active claim");
    if (index === 3) await inspectActivity(page, box);
    await page.screenshot({ path: path.join(output, `${label}-scene-${index + 1}.png`), animations: "disabled" });
  }
  const checkpoints = [];
  for (const local of [0.28, 0.42, 0.58, 0.72, 0.82, 0.28]) {
    await seek(page, 4, local);
    checkpoints.push((await layout(page)).state.checkpoint);
  }
  assert.deepEqual(checkpoints, ["summary", "diff", "evidence", "approval", "merged", "summary"]);
}

async function inspectStatic(options, label) {
  const context = await browser.newContext({ viewport: { width: 390, height: 844 }, ...options });
  const page = await context.newPage();
  await page.goto(base, { waitUntil: "networkidle" });
  const state = await layout(page);
  assert.equal(state.headings.length, 6);
  assert(state.documentWidth <= 391, `${label} overflow`);
  for (const scene of await page.locator("[data-story-scene]").all()) {
    await scene.scrollIntoViewIfNeeded();
    assert(await scene.isVisible(), `${label} must keep each chapter readable`);
  }
  await page.screenshot({ path: path.join(output, `${label}.png`), fullPage: true });
  findings.push({ mode: label, ...state });
  await context.close();
}

try {
  for (const [width, height] of sizes) await inspectViewport(width, height);
  await inspectStatic({ reducedMotion: "reduce" }, "reduced-motion");
  await inspectStatic({ javaScriptEnabled: false }, "no-javascript");
  await fs.writeFile(path.join(output, "browser-results.json"), JSON.stringify(findings, null, 2));
  console.log(`Passed ${findings.length} browser profiles. Artifacts: ${output}`);
} finally {
  await browser.close();
}
