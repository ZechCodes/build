// Exercise the public landing in a browser. Start scripts/preview-landing.py
// first (it serves the generated page and every /landing/ asset).
//
//   LANDING_URL         where the preview listens (default http://127.0.0.1:4173)
//   LANDING_REVIEW_DIR  where captures land (default /tmp/build-landing-review)
//   CHROMIUM_PATH       a system Chromium instead of Playwright's download
//   LANDING_GPU=1       use the machine's GPU through ANGLE instead of SwiftShader
//
// Two versions of the page are checked by exit code: the document (phones,
// reduced motion, no JavaScript) must carry the whole story in order, and the
// film (desktop) must start its stage, reach every act, keep each act's
// headline in the viewport, keep every close-up on its screen after the
// window is resized, and raise no browser errors. On SwiftShader the film is
// forced past its frame budget with ?film=force, because a software renderer
// would otherwise, correctly, hand back to the document.
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { chromium } from "playwright";

const base = process.env.LANDING_URL || "http://127.0.0.1:4173";
const output = process.env.LANDING_REVIEW_DIR || "/tmp/build-landing-review";
const gpu = process.env.LANDING_GPU === "1";
await fs.mkdir(output, { recursive: true });

const browser = await chromium.launch({
  headless: true,
  executablePath: process.env.CHROMIUM_PATH,
  args: gpu
    ? ["--headless=new", "--use-gl=angle", "--use-angle=gl", "--ignore-gpu-blocklist"]
    : ["--enable-unsafe-swiftshader"],
});

const HEADLINES = [
  "Your agents. Your machine. Your call.",
  "The work runs on your machine.",
  "Say what needs doing.",
  "One issue. A whole team.",
  "Build the workflow. Then run it again.",
  "Every change lands in Git.",
  "See what needs you. Decide what ships.",
  "Your work stays put. You don't have to.",
];
const FILM_VIEWPORTS = [[1440, 900], [1920, 1080], [1024, 768]];
const DOCUMENT_VIEWPORTS = [[390, 844], [360, 740]];
const findings = [];

function watchErrors(page) {
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  return errors;
}

async function layout(page) {
  return page.evaluate(() => ({
    width: innerWidth,
    documentWidth: document.documentElement.scrollWidth,
    mode: document.documentElement.dataset.mode || "document",
    stage: document.documentElement.dataset.stage || "",
    headings: [...document.querySelectorAll("[data-act] h1, [data-act] h2")].map((element) => element.textContent.trim()),
    missingImages: [...document.images].filter((image) => image.complete && !image.naturalWidth).map((image) => image.src),
    slots: (document.documentElement.outerHTML.match(/\{\{\w+\}\}/g) || []),
  }));
}

function assertStory(state, label) {
  for (const headline of HEADLINES) assert.ok(state.headings.includes(headline), `${label}: ${headline}`);
  assert.deepEqual(state.slots, [], `${label}: every server slot is filled`);
  assert.deepEqual(state.missingImages, [], `${label}: all visible media should resolve`);
}

async function inspectDocument(width, height, options, label) {
  const context = await browser.newContext({ viewport: { width, height }, ...options });
  const page = await context.newPage();
  const errors = watchErrors(page);
  await page.goto(base, { waitUntil: "networkidle" });
  const state = await layout(page);
  assertStory(state, label);
  assert.equal(state.mode, "document", `${label} reads the document`);
  assert.ok(state.documentWidth <= width + 1, `${label}: no horizontal overflow`);
  for (const act of await page.locator("[data-act]").all()) {
    await act.scrollIntoViewIfNeeded();
    assert.ok(await act.isVisible(), `${label}: each act is readable`);
    assert.ok(await act.locator(".poster img").first().isVisible(), `${label}: each act keeps its still`);
  }
  assert.ok(await page.locator("#waitlist form").isVisible(), `${label}: the waitlist form is reachable`);
  assert.equal(await page.locator("#waitlist button").textContent().then((text) => text.trim()), "Join the waitlist");
  assert.ok(await page.locator('footer a[href="/docs"]').count(), `${label}: the footer reaches the docs`);
  await page.screenshot({ path: path.join(output, `${label}.png`), fullPage: true });
  assert.deepEqual(errors, [], `${label}: browser errors`);
  findings.push({ label, viewport: [width, height], mode: state.mode });
  await context.close();
}

async function seek(page, act, local) {
  await page.evaluate(({ act, local }) => window.BuildFilm.seek(act, local), { act, local });
  // The scrub eases the playhead toward the scroll position.
  await page.waitForFunction(({ act, local }) => {
    const film = window.BuildFilm;
    return Math.abs(film.timeline.time() - film.time()) < 0.5 && document.querySelector("[data-film]").dataset.act === String(act);
  }, { act, local }, { timeout: gpu ? 5000 : 30_000 });
  await page.waitForTimeout(150);
}

// Every close-up that is showing sits inside the window, and the stage's
// idea of the window is the window. After a resize both must still hold.
async function assertAligned(page, label, width, height) {
  const state = await page.evaluate(() => ({
    viewport: window.BuildFilm.stage.getState().viewport,
    window: [innerWidth, innerHeight],
    panels: [...document.querySelectorAll("[data-panel]")]
      .filter((panel) => panel.style.visibility === "visible" && Number(panel.style.opacity) > 0.5)
      .map((panel) => ({ name: panel.dataset.panel, box: panel.getBoundingClientRect().toJSON() })),
  }));
  assert.deepEqual([state.viewport.width, state.viewport.height], [width, height], `${label}: the stage viewport is the window`);
  assert.deepEqual(state.window, [width, height], `${label}: the window is ${width}x${height}`);
  assert.ok(state.panels.length > 0, `${label}: a close-up is showing`);
  for (const { name, box } of state.panels) {
    const inside = box.left >= -1 && box.top >= -1 && box.right <= width + 1 && box.bottom <= height + 1;
    assert.ok(inside, `${label}: the ${name} close-up sits inside the window (${JSON.stringify(box)})`);
  }
}

async function checkAct(page, label, act, local, height) {
  await seek(page, act, local);
  const heading = page.locator(`[data-act="${act}"]`).locator("h1, h2").first();
  const box = await heading.boundingBox();
  assert.ok(box && box.y >= 0 && box.y + box.height <= height, `${label}: act ${act} headline in view`);
  const screens = await page.evaluate(() => window.BuildFilm.stage.getState().screens);
  assert.ok(screens.laptop, `${label}: the laptop shows a display in act ${act}`);
  const lid = await page.evaluate(() => window.BuildFilm.pose.laptop.lidOpen);
  assert.ok(lid > 0.99, `${label}: the laptop never closes (act ${act})`);
  await page.screenshot({ path: path.join(output, `${label}-act-${act}-${local}.png`) });
}

async function openFilm(width, height, label) {
  const context = await browser.newContext({ viewport: { width, height } });
  const page = await context.newPage();
  const errors = watchErrors(page);
  await page.goto(`${base}/?film=${gpu ? "1" : "force"}`, { waitUntil: "networkidle" });
  await page.waitForFunction(() => document.documentElement.dataset.stage === "ready", null, { timeout: 60_000 });
  await page.waitForTimeout(1200);
  const state = await layout(page);
  assertStory(state, label);
  assert.equal(state.mode, "film", `${label} runs the film`);
  assert.ok(state.documentWidth <= width + 1, `${label}: no horizontal overflow`);
  return { context, page, errors, state };
}

async function inspectFilm(width, height) {
  const label = `${width}x${height}-film`;
  const { context, page, errors, state } = await openFilm(width, height, label);
  const opened = await page.evaluate(() => window.BuildFilm.pose.laptop.lidOpen);
  assert.ok(opened > 0.99, `${label}: the hero laptop is open after its entrance`);
  await page.screenshot({ path: path.join(output, `${label}-hero.png`) });
  for (const [act, local] of [[1, 0.5], [2, 0.7], [3, 0.6], [4, 0.7], [5, 0.75], [6, 0.8], [7, 0.55], [7, 0.85], [8, 0.9], [4, 0.2], [1, 0.05]]) {
    await checkAct(page, label, act, local, height);
  }
  await checkAct(page, label, 7, 0.6, height);
  await assertAligned(page, label, width, height);
  const frame = await page.evaluate(() => window.BuildFilm.stage.getState().lastFrameMs);
  await page.locator("#details").scrollIntoViewIfNeeded();
  assert.ok(await page.locator("#details").isVisible(), `${label}: the practical section follows the film`);
  assert.deepEqual(errors, [], `${label}: browser errors`);
  findings.push({ label, viewport: [width, height], mode: state.mode, lastFrameMs: frame });
  await context.close();
}

// Scrubbing back over a labelled change hands back the label that was there
// before it, not the markup's: the sequences Astra reproduced, forward and
// back, plus the caption a finished node keeps and the hunk's count.
const SCRUBS = [
  { name: "act 4 implement status", selector: '[data-row-status="implement"]', steps: [[4, 0.6, "Waiting", true], [4, 0.8, "Working", false], [4, 0.6, "Waiting", true]] },
  { name: "act 5 implement caption", selector: '[data-node-status="implement"]', steps: [[5, 0.5, "Done · handoff pending"], [5, 0.82, "Done"], [5, 0.5, "Done · handoff pending"]] },
  { name: "act 6 hunk count", selector: "[data-diff-add]", steps: [[6, 0.3, "+3"], [6, 0.8, "+4"], [6, 0.3, "+3"]] },
  { name: "act 6 tree label", selector: "[data-tree-label]", steps: [[6, 0.7, "Staged"], [6, 0.9, "Working tree"], [6, 0.7, "Staged"], [6, 0.5, "Working tree"]] },
];

async function inspectScrubs(width, height) {
  const label = `${width}x${height}-scrub`;
  const { context, page, errors, state } = await openFilm(width, height, label);
  for (const { name, selector, steps } of SCRUBS) {
    for (const [act, local, expected, waiting] of steps) {
      await seek(page, act, local);
      const found = await page.evaluate((query) => {
        const element = document.querySelector(query);
        return { text: element.textContent, waiting: element.classList.contains("waiting") };
      }, selector);
      assert.equal(found.text, expected, `${label}: ${name} at ${act}/${local}`);
      if (waiting !== undefined) assert.equal(found.waiting, waiting, `${label}: ${name} waiting class at ${act}/${local}`);
    }
  }
  assert.deepEqual(errors, [], `${label}: browser errors`);
  findings.push({ label, viewport: [width, height], mode: state.mode });
  await context.close();
}

// A desktop window made smaller mid-film: the pin re-measures, the stage
// follows, and the close-ups stay on their screens.
async function inspectResize([fromWidth, fromHeight], [toWidth, toHeight]) {
  const label = `${fromWidth}x${fromHeight}-to-${toWidth}x${toHeight}-film`;
  const { context, page, errors, state } = await openFilm(fromWidth, fromHeight, label);
  await checkAct(page, label, 7, 0.55, fromHeight);
  await page.setViewportSize({ width: toWidth, height: toHeight });
  // ScrollTrigger refreshes on a debounce; give it that and a few frames.
  await page.waitForTimeout(600);
  for (const [act, local] of [[7, 0.55], [6, 0.8], [5, 0.75], [3, 0.6]]) {
    await checkAct(page, label, act, local, toHeight);
    await assertAligned(page, `${label} act ${act}`, toWidth, toHeight);
  }
  assert.deepEqual(errors, [], `${label}: browser errors`);
  findings.push({ label, viewport: [toWidth, toHeight], mode: state.mode });
  await context.close();
}

try {
  for (const [width, height] of DOCUMENT_VIEWPORTS) await inspectDocument(width, height, {}, `${width}x${height}-phone`);
  await inspectDocument(1440, 900, { reducedMotion: "reduce" }, "1440x900-reduced-motion");
  await inspectDocument(1440, 900, { javaScriptEnabled: false }, "1440x900-no-javascript");
  for (const [width, height] of FILM_VIEWPORTS) await inspectFilm(width, height);
  await inspectResize([1440, 900], [1024, 768]);
  await inspectScrubs(1440, 900);
  await fs.writeFile(path.join(output, "browser-results.json"), JSON.stringify(findings, null, 2));
  console.log(`Passed ${findings.length} browser profiles. Artifacts: ${output}`);
} finally {
  await browser.close();
}
