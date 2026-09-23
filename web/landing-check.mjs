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
  assert.ok(await page.locator(".site-nav .cta").isVisible(), `${label}: the bar's call to action is on screen`);
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

// A storyboard beat: the wheel at the act's position and the act's scene,
// which otherwise runs on the clock, held at the same local progress.
async function checkAct(page, label, act, local, height) {
  await seek(page, act, local);
  await page.evaluate(({ act, local }) => window.BuildFilm.sceneSeek(act, local), { act, local });
  await page.waitForTimeout(100);
  // The film root carries the current act as data-act too; the id is the act's.
  const heading = page.locator(`#act-${act}-title`);
  const box = await heading.boundingBox();
  assert.ok(box && box.y >= 0 && box.y + box.height <= height, `${label}: act ${act} headline in view`);
  const screens = await page.evaluate(() => window.BuildFilm.stage.getState().screens);
  assert.ok(screens.laptop, `${label}: the laptop shows a display in act ${act}`);
  const lid = await page.evaluate(() => window.BuildFilm.pose.laptop.lidOpen);
  assert.ok(lid > 0.99, `${label}: the laptop never closes (act ${act})`);
  await page.screenshot({ path: path.join(output, `${label}-act-${act}-${local}.png`) });
}

async function openFilm(width, height, label, query = "") {
  const context = await browser.newContext({ viewport: { width, height } });
  const page = await context.newPage();
  const errors = watchErrors(page);
  await page.goto(`${base}/?film=${gpu ? "1" : "force"}${query}`, { waitUntil: "networkidle" });
  await page.waitForFunction(() => document.documentElement.dataset.stage === "ready", null, { timeout: 60_000 });
  await page.waitForTimeout(1200);
  const state = await layout(page);
  assertStory(state, label);
  assert.equal(state.mode, "film", `${label} runs the film`);
  assert.ok(state.documentWidth <= width + 1, `${label}: no horizontal overflow`);
  return { context, page, errors, state };
}

// The primary call to action, hit-tested the way a visitor reaches it: the
// element under the pointer at the button's centre, then a real click that
// must move the film. An invisible copy container of a later act sitting over
// the hero is what this catches; an element-exists check would not.
async function checkHeroPointerPath(page, label) {
  const cta = page.locator("#act-1 .actions .cta");
  const box = await cta.boundingBox();
  assert.ok(box, `${label}: the hero call to action has a box`);
  const x = box.x + box.width / 2;
  const y = box.y + box.height / 2;
  const under = await page.evaluate(([px, py]) => {
    const hit = document.elementFromPoint(px, py);
    return { tag: hit?.tagName, text: hit?.textContent?.trim(), act: hit?.closest(".act")?.id, isCta: !!hit?.closest("#act-1 .actions .cta") };
  }, [x, y]);
  assert.ok(under.isCta, `${label}: the hero call to action is under the pointer (${JSON.stringify(under)})`);
  await page.mouse.click(x, y);
  await page.waitForFunction(() => document.querySelector("[data-film]").dataset.act === "8", null, { timeout: gpu ? 10_000 : 30_000 });
  // Where the click lands, the form takes the pointer too.
  await page.waitForFunction(() => {
    const field = document.querySelector("#act-8 form input");
    const rect = field?.getBoundingClientRect();
    if (!rect || rect.width === 0) return false;
    return document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2) === field;
  }, null, { timeout: gpu ? 10_000 : 30_000 });
  await page.screenshot({ path: path.join(output, `${label}-hero-cta-click.png`) });
  await seek(page, 1, 0);
  await page.waitForTimeout(400);
}

async function inspectFilm(width, height) {
  const label = `${width}x${height}-film`;
  const { context, page, errors, state } = await openFilm(width, height, label);
  // The lid entrance is a timed tween; under a loaded software renderer GSAP
  // smooths long frames by slowing its clock, so wait for the lid, not a delay.
  await page.waitForFunction(() => window.BuildFilm.pose.laptop.lidOpen > 0.99, null, { timeout: gpu ? 5000 : 30_000 })
    .catch(() => {});
  const opened = await page.evaluate(() => window.BuildFilm.pose.laptop.lidOpen);
  assert.ok(opened > 0.99, `${label}: the hero laptop is open after its entrance`);
  await page.screenshot({ path: path.join(output, `${label}-hero.png`) });
  await checkHeroPointerPath(page, label);
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

// The clock, not the wheel: entering an act brings its copy in and plays its
// scene through on its own; leaving it backwards rewinds both; a beat can be
// held for a check. The label sequences Astra reproduced in round 1 run on
// the scene's clock now.
const SCRUBS = [
  { name: "act 4 implement status", act: 4, selector: '[data-row-status="implement"]', steps: [[0.6, "Waiting", true], [0.82, "Working", false], [0.6, "Waiting", true]] },
  { name: "act 5 implement caption", act: 5, selector: '[data-node-status="implement"]', steps: [[0.5, "Done · handoff pending"], [0.82, "Done"], [0.5, "Done · handoff pending"]] },
  { name: "act 6 hunk count", act: 6, selector: "[data-diff-add]", steps: [[0.3, "+3"], [0.8, "+4"], [0.3, "+3"]] },
  { name: "act 6 tree label", act: 6, selector: "[data-tree-label]", steps: [[0.7, "Staged"], [0.9, "Working tree"], [0.7, "Staged"], [0.5, "Working tree"]] },
];

const sceneTimeout = () => (gpu ? 20_000 : 60_000);

async function opacityOf(page, selector) {
  return page.evaluate((query) => getComputedStyle(document.querySelector(query)).opacity, selector);
}

async function waitForOpacity(page, selector, value) {
  await page.waitForFunction(({ query, value }) => getComputedStyle(document.querySelector(query)).opacity === value, { query: selector, value }, { timeout: 5000 });
}

async function inspectScenes(width, height) {
  const label = `${width}x${height}-scenes`;
  const { context, page, errors, state } = await openFilm(width, height, label);
  // Into act 3: the copy comes in and the scene plays to its end by itself.
  await seek(page, 3, 0.5);
  await page.waitForFunction(() => window.BuildFilm.scenes[3].isActive(), null, { timeout: 5000 });
  await waitForOpacity(page, "#act-3-title", "1");
  await waitForOpacity(page, "#act-2-title", "0");
  await page.waitForFunction(() => window.BuildFilm.scenes[3].progress() === 1, null, { timeout: sceneTimeout() });
  assert.equal(await opacityOf(page, '[data-column="progress"]'), "1", `${label}: the card reached In progress on the clock`);
  await page.screenshot({ path: path.join(output, `${label}-act-3-played.png`) });
  // Back into act 2: act 3 rewinds and its copy leaves.
  await seek(page, 2, 0.5);
  await waitForOpacity(page, "#act-3-title", "0");
  await waitForOpacity(page, "#act-2-title", "1");
  assert.equal(await page.evaluate(() => window.BuildFilm.scenes[3].progress()), 0, `${label}: act 3's scene rewound`);
  assert.equal(await opacityOf(page, '[data-column="ready"]'), "1", `${label}: the card is back in Ready`);
  // On to act 4: the scene runs through and Implement is back at work.
  await seek(page, 4, 0.5);
  await page.waitForFunction(() => window.BuildFilm.scenes[4].progress() === 1, null, { timeout: sceneTimeout() });
  const status = await readLabel(page, '[data-row-status="implement"]');
  assert.deepEqual(status, { text: "Working", waiting: false }, `${label}: act 4 finished on the clock`);
  await checkReadingHolds(page, label);
  await checkPhoneBeforeQuestion(page, label);
  await checkMergeUncovered(page, label);
  await checkClosingBeats(page, label);
  for (const { name, act, selector, steps } of SCRUBS) {
    await seek(page, act, 0.5);
    for (const [local, expected, waiting] of steps) {
      await page.evaluate(({ act, local }) => window.BuildFilm.sceneSeek(act, local), { act, local });
      const found = await readLabel(page, selector);
      assert.equal(found.text, expected, `${label}: ${name} at ${act}/${local}`);
      if (waiting !== undefined) assert.equal(found.waiting, waiting, `${label}: ${name} waiting class at ${act}/${local}`);
    }
  }
  assert.deepEqual(errors, [], `${label}: browser errors`);
  findings.push({ label, viewport: [width, height], mode: state.mode });
  await context.close();
}

// Round 2's review, item by item, with the wheel stopped where a visitor
// would stop it and no sceneSeek: the scene has to get there on its own.
async function scenePlayed(page, act) {
  await page.waitForFunction((n) => window.BuildFilm.scenes[n].progress() === 1, act, { timeout: sceneTimeout() });
}

function panelOpacity(page, name) {
  return page.evaluate((panel) => {
    const element = document.querySelector(`[data-panel="${panel}"]`);
    return element.style.visibility === "visible" ? Number(element.style.opacity) : 0;
  }, name);
}

// Act 3's card keeps its branch and stays lifted while the visitor stays;
// leaving plays the return. A modest overshoot during playback, still inside
// the act, neither restarts nor cuts the scene.
async function checkReadingHolds(page, label) {
  await seek(page, 2, 0.5);
  await seek(page, 3, 0.3);
  await page.waitForFunction(() => window.BuildFilm.scenes[3].isActive(), null, { timeout: 5000 });
  await seek(page, 3, 0.55);
  assert.ok(await page.evaluate(() => window.BuildFilm.scenes[3].progress() > 0), `${label}: an overshoot inside act 3 keeps its scene going`);
  await scenePlayed(page, 3);
  await page.waitForTimeout(1500);
  assert.equal(await opacityOf(page, "[data-branch-line]"), "1", `${label}: act 3's branch stays open while the visitor stays`);
  await seek(page, 3, 0.95);
  await waitForOpacity(page, "[data-branch-line]", "0");
  await page.screenshot({ path: path.join(output, `${label}-act-3-departed.png`) });
}

// Act 4's phone is on screen before its question is asked.
async function checkPhoneBeforeQuestion(page, label) {
  await seek(page, 3, 0.5);
  await seek(page, 4, 0.23);
  await page.waitForFunction(() => window.BuildFilm.scenes[4].isActive(), null, { timeout: 5000 });
  const phone = await page.evaluate(() => window.BuildFilm.pose.phone.opacity);
  assert.ok(phone > 0.99, `${label}: the phone is in place as act 4's scene starts (${phone})`);
  await page.screenshot({ path: path.join(output, `${label}-act-4-question.png`) });
  await scenePlayed(page, 4);
  await page.screenshot({ path: path.join(output, `${label}-act-4-finished.png`) });
}

// Act 7's approved panel gives way to the merge on the clock, and comes back
// on a replay.
async function checkMergeUncovered(page, label) {
  await seek(page, 6, 0.5);
  await seek(page, 7, 0.6);
  await page.waitForFunction(() => window.BuildFilm.scenes[7].isActive(), null, { timeout: 5000 });
  await scenePlayed(page, 7);
  await page.waitForTimeout(300);
  assert.equal(await panelOpacity(page, "review"), 0, `${label}: the approved panel is off the merge`);
  const screens = await page.evaluate(() => window.BuildFilm.stage.getState().screens);
  assert.match(String(screens.tablet), /merged/, `${label}: the tablet shows the merge`);
  await page.screenshot({ path: path.join(output, `${label}-act-7-merged.png`) });
  await seek(page, 6, 0.5);
  await seek(page, 7, 0.6);
  await page.waitForFunction(() => window.BuildFilm.scenes[7].isActive(), null, { timeout: 5000 });
  assert.ok(await panelOpacity(page, "review") > 0.5, `${label}: a replay brings the review panel back`);
}

// Act 8's two beats: a jump from an early act, or a quick crossing, ends on
// one beat, never both.
async function checkClosingBeats(page, label) {
  const beats = () => page.evaluate(() => ["a", "b"].map((beat) => Number(getComputedStyle(document.querySelector(`[data-beat="${beat}"]`)).opacity)));
  await seek(page, 2, 0.5);
  await seek(page, 8, 0.9);
  await page.waitForTimeout(1500);
  assert.deepEqual(await beats(), [0, 1], `${label}: a jump to 8/.9 shows only the second beat`);
  await seek(page, 8, 0.3);
  await page.waitForTimeout(1500);
  assert.deepEqual(await beats(), [1, 0], `${label}: back to 8/.3 shows only the first beat`);
  await seek(page, 8, 0.7);
  await seek(page, 8, 0.4);
  await seek(page, 8, 0.8);
  await page.waitForTimeout(1500);
  assert.deepEqual(await beats(), [0, 1], `${label}: quick crossings end on one beat`);
  await page.screenshot({ path: path.join(output, `${label}-act-8.png`) });
}

// The first paint is already the film's layout: the hero's copy and call to
// action readable and on top, the cutout where the laptop will be, and no
// document grid first. A film that never starts gives the page back.
async function inspectStartup(width, height) {
  const label = `${width}x${height}-startup`;
  const context = await browser.newContext({ viewport: { width, height } });
  const page = await context.newPage();
  const errors = watchErrors(page);
  const started = Date.now();
  await page.goto(`${base}/?film=${gpu ? "1" : "force"}`, { waitUntil: "commit" });
  await page.waitForSelector("#act-1-title", { state: "attached" });
  for (const at of [100, 400, 1000, 2000]) {
    await page.waitForTimeout(Math.max(0, at - (Date.now() - started)));
    const frame = await page.evaluate(() => {
      const title = document.querySelector("#act-1-title").getBoundingClientRect();
      const cta = document.querySelector("#act-1 .actions .cta").getBoundingClientRect();
      const hit = document.elementFromPoint(cta.x + cta.width / 2, cta.y + cta.height / 2);
      return {
        mode: document.documentElement.dataset.mode,
        titleOpacity: getComputedStyle(document.querySelector("#act-1-title")).opacity,
        titleLeft: title.left,
        ctaOnTop: !!hit?.closest("#act-1 .actions .cta"),
        grid: getComputedStyle(document.querySelector("#act-1 .act__inner")).display,
      };
    });
    assert.equal(frame.mode, "film", `${label} ${at}ms: the film's layout from the first paint`);
    assert.equal(frame.titleOpacity, "1", `${label} ${at}ms: the hero headline is readable`);
    assert.ok(frame.ctaOnTop, `${label} ${at}ms: the call to action takes the pointer`);
    assert.equal(frame.grid, "block", `${label} ${at}ms: no document grid`);
    await page.screenshot({ path: path.join(output, `${label}-${at}ms.png`) });
  }
  assert.deepEqual(errors, [], `${label}: browser errors`);
  await context.close();

  // The film's module slow to arrive: nothing of the film's covers the hero
  // while it waits, and a link to an act pressed meanwhile (the hero's call
  // to action, the bar's, "See how it works") is honoured once the film
  // starts.
  for (const [name, selector, act] of [["hero-cta", "#act-1 .actions .cta", 8], ["nav-cta", ".site-nav .cta", 8], ["see-how", '#act-1 .actions a[href="#act-2"]', 2]]) {
    const slow = await browser.newContext({ viewport: { width, height } });
    const slowPage = await slow.newPage();
    const slowErrors = watchErrors(slowPage);
    await slowPage.route(/\/_astro\/.*\.js$/, async (route) => {
      await new Promise((resolve) => setTimeout(resolve, 3500));
      await route.continue();
    });
    await slowPage.goto(`${base}/?film=${gpu ? "1" : "force"}`, { waitUntil: "commit" });
    await slowPage.waitForFunction(() => document.documentElement.dataset.mode === "film", null, { timeout: 10_000 });
    await slowPage.waitForSelector(selector);
    const pending = await slowPage.evaluate(() => ({
      stage: document.documentElement.dataset.stage,
      overlays: getComputedStyle(document.querySelector("[data-overlays]")).display,
      panels: [...document.querySelectorAll("[data-panel]")].filter((panel) => getComputedStyle(panel).visibility !== "hidden").map((panel) => panel.dataset.panel),
    }));
    assert.equal(pending.stage, "pending", `${label} ${name}: the module is still on its way`);
    assert.deepEqual(pending.panels, [], `${label} ${name}: no close-up shows before the film places it (overlays ${pending.overlays})`);
    if (name === "hero-cta") await slowPage.screenshot({ path: path.join(output, `${label}-pending.png`) });
    await slowPage.locator(selector).click();
    await slowPage.waitForFunction(() => document.documentElement.dataset.stage === "ready", null, { timeout: 60_000 });
    await slowPage.waitForFunction((n) => document.querySelector("[data-film]").dataset.act === String(n), act, { timeout: gpu ? 10_000 : 30_000 });
    if (act === 8) {
      await slowPage.waitForFunction(() => {
        const field = document.querySelector("#act-8 form input");
        const rect = field?.getBoundingClientRect();
        if (!rect || rect.width === 0) return false;
        return document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2) === field;
      }, null, { timeout: gpu ? 10_000 : 30_000 });
    } else {
      await waitForOpacity(slowPage, `#act-${act}-title`, "1");
    }
    await slowPage.screenshot({ path: path.join(output, `${label}-early-${name}.png`) });
    assert.deepEqual(slowErrors, [], `${label} ${name}: browser errors with a slow module`);
    await slow.close();
  }

  // The film's module blocked: the boot's deadline hands the page back.
  const blocked = await browser.newContext({ viewport: { width, height } });
  const blockedPage = await blocked.newPage();
  await blockedPage.route(/\/_astro\/.*\.js$/, (route) => route.abort());
  await blockedPage.goto(`${base}/?film=${gpu ? "1" : "force"}`, { waitUntil: "commit" });
  // First the boot chooses the film, then its deadline gives the page back.
  await blockedPage.waitForFunction(() => document.documentElement.dataset.mode === "film", null, { timeout: 10_000 });
  await blockedPage.locator("#act-1 .actions .cta").click();
  await blockedPage.waitForFunction(() => !document.documentElement.dataset.mode, null, { timeout: 15_000 });
  await blockedPage.waitForTimeout(300);
  const landed = await blockedPage.evaluate(() => {
    const rect = document.querySelector("#act-8 form").getBoundingClientRect();
    return rect.top < innerHeight && rect.bottom > 0;
  });
  assert.ok(landed, `${label}: the call to action pressed while pending lands on the form in the document`);
  assert.ok(await blockedPage.locator("#act-8 form").isVisible() || await blockedPage.locator("#act-8").count(), `${label}: the document is back`);
  await blockedPage.locator("#act-4-title").scrollIntoViewIfNeeded();
  assert.ok(await blockedPage.locator("#act-4-title").isVisible(), `${label}: a later act is readable after a blocked film`);
  await blocked.close();
  findings.push({ label, viewport: [width, height], mode: "film" });
}

// The bar: on screen with its call to action in both variants; the hero-only
// variant leaves after the hero without moving the story.
async function inspectNav(width, height) {
  for (const variant of ["persistent", "hero"]) {
    const label = `${width}x${height}-nav-${variant}`;
    const { context, page, errors, state } = await openFilm(width, height, label, variant === "hero" ? "&nav=hero" : "");
    const nav = page.locator(".site-nav");
    assert.ok(await nav.isVisible(), `${label}: the bar is there on the hero`);
    const before = await page.locator("#act-4-title").evaluate(() => document.querySelector(".act[data-act='4'] .act__copy").getBoundingClientRect().top);
    await seek(page, 4, 0.5);
    await page.waitForTimeout(500);
    const after = await page.locator("#act-4-title").evaluate(() => document.querySelector(".act[data-act='4'] .act__copy").getBoundingClientRect().top);
    assert.equal(before, after, `${label}: the story sits in the same place`);
    assert.equal(await nav.isVisible(), variant === "persistent", `${label}: the bar in act 4`);
    await page.screenshot({ path: path.join(output, `${label}-act-4.png`) });
    if (variant === "persistent") {
      await page.locator(".site-nav .cta").click();
      await page.waitForFunction(() => document.querySelector("[data-film]").dataset.act === "8", null, { timeout: gpu ? 10_000 : 30_000 });
    }
    assert.deepEqual(errors, [], `${label}: browser errors`);
    findings.push({ label, viewport: [width, height], mode: state.mode });
    await context.close();
  }
}

async function readLabel(page, selector) {
  return page.evaluate((query) => {
    const element = document.querySelector(query);
    return { text: element.textContent, waiting: element.classList.contains("waiting") };
  }, selector);
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
  await inspectScenes(1440, 900);
  await inspectStartup(1440, 900);
  await inspectNav(1440, 900);
  await fs.writeFile(path.join(output, "browser-results.json"), JSON.stringify(findings, null, 2));
  console.log(`Passed ${findings.length} browser profiles. Artifacts: ${output}`);
} finally {
  await browser.close();
}
