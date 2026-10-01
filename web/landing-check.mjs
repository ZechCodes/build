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
  "Your agents are moving fast. Know what needs you.",
  "See your agents in one place.",
  "Write the task. Hand it off.",
  "See which agent needs an answer.",
  "Ask another agent to review it.",
  "Make the last edit yourself.",
  "Read the diff. Make the call.",
  "Answer from your phone.",
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
  // A phone's first visit plays the hero's entrance; the document is read
  // once it is still.
  if (options.javaScriptEnabled !== false) await waitForEntrance(page, label);
  const state = await layout(page);
  assertStory(state, label);
  assert.equal(state.mode, "document", `${label} reads the document`);
  assert.ok(state.documentWidth <= width + 1, `${label}: no horizontal overflow`);
  for (const act of await page.locator("[data-act]").all()) {
    await act.scrollIntoViewIfNeeded();
    assert.ok(await act.isVisible(), `${label}: each act is readable`);
    assert.ok(await act.locator(".poster img, .hero-device img").first().isVisible(), `${label}: each act keeps its still`);
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

// An act's resting point, where its copy and beat are.
async function rest(page, act) {
  await page.evaluate((n) => window.BuildFilm.rest(n), act);
  await page.waitForFunction((n) => {
    const film = window.BuildFilm;
    return Math.abs(film.timeline.time() - film.time()) < 0.5 && document.querySelector("[data-film]").dataset.act === String(n);
  }, act, { timeout: gpu ? 5000 : 60_000 });
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

// A storyboard beat: the wheel at the act's resting point and the act's
// scene, which otherwise runs on the clock, held at a local progress.
async function checkAct(page, label, act, local, height) {
  await rest(page, act);
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

// The film's own checks open on the hero at rest (?hero=0); the entrance
// has its own below, and inspectFilm plays it before the film is checked.
async function openFilm(width, height, label, query = "&hero=0") {
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

// The calls to action, hit-tested the way a visitor reaches them: the
// element under the pointer at each one's centre must be that link. An
// invisible copy container of a later act sitting over the hero, or a
// decorative layer, is what this catches; an element-exists check would not.
async function assertOnTop(page, selector, label) {
  const under = await page.evaluate((query) => {
    const element = document.querySelector(query);
    const box = element?.getBoundingClientRect();
    if (!box || box.width === 0) return { missing: true };
    const hit = document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2);
    return { onTop: Boolean(hit?.closest(query)), tag: hit?.tagName, href: element.getAttribute("href") };
  }, selector);
  assert.ok(under.onTop, `${label}: ${selector} is under the pointer (${JSON.stringify(under)})`);
  return under;
}

async function checkHeroPointerPath(page, label) {
  const hero = await assertOnTop(page, "#act-1 .actions .cta", label);
  assert.equal(hero.href, "/docs#setup", `${label}: the hero's download goes to the installers`);
  const nav = await assertOnTop(page, ".site-nav .cta", label);
  assert.equal(nav.href, "/docs#setup", `${label}: the bar's download goes to the installers`);
  await assertOnTop(page, "#act-1 .actions a[title]", label);
}

// The desktop film, after the entrance has played: the stage drew the
// laptop's turn when it was ready in time, and the hero rests on it.
async function inspectFilm(width, height) {
  const label = `${width}x${height}-film`;
  const { context, page, errors, state } = await openFilm(width, height, label, "");
  await waitForEntrance(page, label);
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

// The clock, not the wheel: reaching an act's resting point brings its copy
// in and plays its beat through on its own in BEAT_SECONDS; an act is
// finished whenever the visitor comes back to it; a beat can be held for a
// check. The label sequences Astra reproduced in round 1 run on the scene's
// clock.
const SCRUBS = [
  { name: "act 4 implement status", act: 4, selector: '[data-row-status="implement"]', steps: [[0.6, "Waiting", true], [0.82, "Working", false], [0.6, "Waiting", true]] },
  { name: "act 5 implement caption", act: 5, selector: '[data-node-status="implement"]', steps: [[0.5, "Done · handoff pending"], [0.82, "Done"], [0.5, "Done · handoff pending"]] },
  { name: "act 6 hunk count", act: 6, selector: "[data-diff-add]", steps: [[0.3, "+3"], [0.8, "+4"], [0.3, "+3"]] },
  { name: "act 6 tree label", act: 6, selector: "[data-tree-label]", steps: [[0.7, "Staged"], [0.9, "Uncommitted"], [0.7, "Staged"], [0.5, "Uncommitted"]] },
  { name: "act 6 uncommitted weight", act: 6, selector: "[data-tree-count]", steps: [[0.3, "+6 −1"], [0.6, "+7 −1"], [0.9, "clean"], [0.6, "+7 −1"], [0.3, "+6 −1"]] },
  { name: "act 6 diff total", act: 6, selector: '.diff-bar [data-total="add"]', steps: [[0.3, "+6"], [0.6, "+7"], [0.9, "+0"], [0.3, "+6"]] },
  { name: "act 6 file count", act: 6, selector: "[data-total-files]", steps: [[0.6, "2 files"], [0.9, "0 files"], [0.6, "2 files"]] },
];

// A beat runs on GSAP's clock, which lag smoothing slows whenever a frame
// takes over half a second: on a loaded SwiftShader box a 5 s beat can take
// minutes of wall time while running correctly.
const sceneTimeout = () => (gpu ? 20_000 : 180_000);

async function opacityOf(page, selector) {
  return page.evaluate((query) => getComputedStyle(document.querySelector(query)).opacity, selector);
}

async function waitForOpacity(page, selector, value) {
  await page.waitForFunction(({ query, value }) => getComputedStyle(document.querySelector(query)).opacity === value, { query: selector, value }, { timeout: 5000 });
}

async function beatStarted(page, act) {
  await page.waitForFunction((n) => window.BuildFilm.playing(n), act, { timeout: 5000 });
}

async function beatFinished(page, act) {
  await page.waitForFunction((n) => !window.BuildFilm.playing(n) && window.BuildFilm.beats[n].progress() === 1, act, { timeout: sceneTimeout() });
}

async function inspectScenes(width, height) {
  const label = `${width}x${height}-scenes`;
  const { context, page, errors, state } = await openFilm(width, height, label);
  // Into act 3: the copy comes in and the beat plays to its end by itself.
  await rest(page, 3);
  await beatStarted(page, 3);
  await waitForOpacity(page, "#act-3-title", "1");
  await waitForOpacity(page, "#act-2-title", "0");
  await beatFinished(page, 3);
  assert.equal(await opacityOf(page, '[data-column="progress"]'), "1", `${label}: the card reached In progress on the clock`);
  await page.screenshot({ path: path.join(output, `${label}-act-3-played.png`) });
  await checkPhoneBeforeQuestion(page, label);
  await checkFinishedOnReturn(page, label);
  await checkMergeUncovered(page, label);
  await checkClosingBeats(page, label);
  for (const { name, act, selector, steps } of SCRUBS) {
    await rest(page, act);
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

function panelOpacity(page, name) {
  return page.evaluate((panel) => {
    const element = document.querySelector(`[data-panel="${panel}"]`);
    return element.style.visibility === "visible" ? Number(element.style.opacity) : 0;
  }, name);
}

// Act 4's phone is on screen before its question is asked, and the beat
// ends with Implement back at work.
async function checkPhoneBeforeQuestion(page, label) {
  await rest(page, 4);
  await beatStarted(page, 4);
  const phone = await page.evaluate(() => window.BuildFilm.pose.phone.opacity);
  assert.ok(phone > 0.99, `${label}: the phone is in place as act 4's beat starts (${phone})`);
  await page.screenshot({ path: path.join(output, `${label}-act-4-question.png`) });
  await beatFinished(page, 4);
  const status = await readLabel(page, '[data-row-status="implement"]');
  assert.deepEqual(status, { text: "Working", waiting: false }, `${label}: act 4 finished on the clock`);
  await page.screenshot({ path: path.join(output, `${label}-act-4-finished.png`) });
}

// Back to an act already seen: it is finished at once, not replayed, and its
// card keeps its branch while the visitor stays; leaving plays the return.
// Further back, the act before is finished too.
async function checkFinishedOnReturn(page, label) {
  await rest(page, 3);
  assert.equal(await page.evaluate(() => window.BuildFilm.playing(3)), false, `${label}: act 3 does not replay`);
  assert.equal(await page.evaluate(() => window.BuildFilm.beats[3].progress()), 1, `${label}: act 3 is finished on return`);
  await waitForOpacity(page, "#act-3-title", "1");
  await page.waitForTimeout(1500);
  assert.equal(await opacityOf(page, "[data-branch-line]"), "1", `${label}: act 3's branch stays open while the visitor stays`);
  await rest(page, 2);
  await waitForOpacity(page, "#act-3-title", "0");
  await waitForOpacity(page, "#act-2-title", "1");
  assert.equal(await page.evaluate(() => window.BuildFilm.beats[2].progress()), 1, `${label}: act 2 is finished on return`);
  await rest(page, 3);
  await page.evaluate(() => window.BuildFilm.seek(3, 0.6));
  await waitForOpacity(page, "[data-branch-line]", "0");
  await page.screenshot({ path: path.join(output, `${label}-act-3-departed.png`) });
}

// Act 7's approved panel gives way to the merge on the clock, and the merge
// is what a return shows.
async function checkMergeUncovered(page, label) {
  await rest(page, 6);
  await rest(page, 7);
  await beatStarted(page, 7);
  await beatFinished(page, 7);
  await page.waitForTimeout(300);
  assert.equal(await panelOpacity(page, "review"), 0, `${label}: the approved panel is off the merge`);
  const screens = await page.evaluate(() => window.BuildFilm.stage.getState().screens);
  assert.match(String(screens.tablet), /merged/, `${label}: the tablet shows the merge`);
  await page.screenshot({ path: path.join(output, `${label}-act-7-merged.png`) });
  await rest(page, 6);
  await rest(page, 7);
  await page.waitForTimeout(300);
  assert.equal(await panelOpacity(page, "review"), 0, `${label}: a return shows the merge, not a replay`);
}

// Act 8's two beats: arriving plays the first and ends on the second; the
// move back toward act 7 takes both; a return, or quick crossings, end on
// the second alone.
async function checkClosingBeats(page, label) {
  // What a visitor sees of each beat: the element's opacity times its
  // headline's, since the scroll fades one and the beat the other.
  const beats = () => page.evaluate(() => ["a", "b"].map((beat) => {
    const element = document.querySelector(`[data-beat="${beat}"]`);
    return Number(getComputedStyle(element).opacity) * Number(getComputedStyle(element.querySelector("h2")).opacity);
  }));
  await rest(page, 2);
  await rest(page, 8);
  await beatFinished(page, 8);
  await page.waitForTimeout(300);
  assert.deepEqual(await beats(), [0, 1], `${label}: act 8 settles on its second beat`);
  await rest(page, 7);
  await page.waitForTimeout(1000);
  assert.deepEqual(await beats(), [0, 0], `${label}: back in act 7, neither beat shows`);
  await rest(page, 8);
  await page.waitForTimeout(300);
  assert.deepEqual(await beats(), [0, 1], `${label}: a return shows the second beat`);
  // A jump back that the playhead crosses in one slow frame, as a software
  // renderer does: the scroll's fade and the beat's rewind must not leave a
  // beat on. The frame is held just under GSAP's lag-smoothing threshold,
  // so the scrub covers most of the jump in one tick.
  await page.evaluate(() => {
    window.BuildFilm.rest(7);
    const started = performance.now();
    while (performance.now() - started < 450);
  });
  await page.waitForTimeout(500);
  assert.deepEqual(await beats(), [0, 0], `${label}: a one-frame jump back to act 7 shows neither beat`);
  await rest(page, 8);
  await rest(page, 7);
  await rest(page, 8);
  await rest(page, 6);
  await rest(page, 8);
  await page.waitForTimeout(1000);
  assert.deepEqual(await beats(), [0, 1], `${label}: quick crossings end on one beat`);
  await page.screenshot({ path: path.join(output, `${label}-act-8.png`) });
}

// The first paint is already the film's layout: on a visit that does not
// play the entrance, the hero's copy and call to action readable and on top,
// the picture where the laptop will be, and no document grid first. A film
// that never starts gives the page back.
async function inspectStartup(width, height) {
  const label = `${width}x${height}-startup`;
  await checkFirstPaint(width, height, label);
  await checkSlowModule(width, height, label);
  await checkBlockedModule(width, height, label);
  await checkFailedHardware(width, height, label);
  findings.push({ label, viewport: [width, height], mode: "film" });
}

async function checkFirstPaint(width, height, label) {
  const context = await browser.newContext({ viewport: { width, height } });
  const page = await context.newPage();
  const errors = watchErrors(page);
  const started = Date.now();
  await page.goto(`${base}/?film=${gpu ? "1" : "force"}&hero=0`, { waitUntil: "commit" });
  await page.waitForSelector("#act-1-title", { state: "attached" });
  for (const at of [100, 400, 1000, 2000]) {
    await page.waitForTimeout(Math.max(0, at - (Date.now() - started)));
    const frame = await page.evaluate(() => ({
      mode: document.documentElement.dataset.mode,
      titleOpacity: getComputedStyle(document.querySelector("#act-1-title")).opacity,
      grid: getComputedStyle(document.querySelector("#act-1 .act__inner")).display,
      picture: getComputedStyle(document.querySelector("[data-hero-device]")).position,
    }));
    assert.equal(frame.mode, "film", `${label} ${at}ms: the film's layout from the first paint`);
    assert.equal(frame.titleOpacity, "1", `${label} ${at}ms: the hero headline is readable`);
    assert.equal(frame.grid, "block", `${label} ${at}ms: no document grid`);
    assert.equal(frame.picture, "absolute", `${label} ${at}ms: the laptop picture stands where the stage will draw`);
    await assertOnTop(page, "#act-1 .actions .cta", `${label} ${at}ms`);
    await page.screenshot({ path: path.join(output, `${label}-${at}ms.png`) });
  }
  assert.deepEqual(errors, [], `${label}: browser errors`);
  await context.close();
}

const delayModules = (page, ms) => page.route(/\/_astro\/.*\.js$/, async (route) => {
  await new Promise((resolve) => setTimeout(resolve, ms));
  await route.continue();
});

// The page's modules slow to arrive. The field is full and moving before
// any of them (it drifts on CSS), the bar's download takes the pointer,
// nothing of the film's covers the hero meanwhile, and the entrance still
// plays once they come. A link to an act opened while the film is on its
// way lands there once the film starts, and skips the entrance.
async function checkSlowModule(width, height, label) {
  const slow = await browser.newContext({ viewport: { width, height } });
  const slowPage = await slow.newPage();
  const slowErrors = watchErrors(slowPage);
  await delayModules(slowPage, 3500);
  await slowPage.goto(`${base}/?film=${gpu ? "1" : "force"}`, { waitUntil: "commit" });
  await slowPage.waitForFunction(() => document.documentElement.dataset.mode === "film", null, { timeout: 10_000 });
  await slowPage.waitForSelector(".hero-pill");
  const lane = () => slowPage.evaluate(() => new DOMMatrixReadOnly(getComputedStyle(document.querySelector(".hero-lane--far")).transform).m41);
  const first = await lane();
  await slowPage.waitForTimeout(300);
  const pending = await slowPage.evaluate(() => ({
    stage: document.documentElement.dataset.stage,
    hero: document.documentElement.dataset.hero,
    panels: [...document.querySelectorAll("[data-panel]")].filter((panel) => getComputedStyle(panel).visibility !== "hidden").map((panel) => panel.dataset.panel),
  }));
  assert.ok(await lane() > first, `${label}: the field moves before any script but the boot's`);
  assert.equal(pending.stage, "pending", `${label}: the module is still on its way`);
  assert.equal(pending.hero, "entrance", `${label}: the entrance is waiting for its module`);
  assert.deepEqual(pending.panels, [], `${label}: no close-up shows before the film places it`);
  await assertOnTop(slowPage, ".site-nav .cta", `${label} pending`);
  await slowPage.screenshot({ path: path.join(output, `${label}-pending.png`) });
  await slowPage.waitForFunction(() => window.BuildHero !== undefined, null, { timeout: 30_000 });
  await waitForEntrance(slowPage, `${label} slow`);
  assert.deepEqual(slowErrors, [], `${label}: browser errors with a slow module`);
  await slow.close();

  const linked = await browser.newContext({ viewport: { width, height } });
  const linkedPage = await linked.newPage();
  await delayModules(linkedPage, 3500);
  await linkedPage.goto(`${base}/?film=${gpu ? "1" : "force"}#act-8`, { waitUntil: "commit" });
  await linkedPage.waitForFunction(() => document.documentElement.dataset.mode === "film", null, { timeout: 10_000 });
  assert.equal(await linkedPage.evaluate(() => document.documentElement.dataset.hero), undefined, `${label}: a link to act 8 skips the entrance`);
  await linkedPage.waitForFunction(() => document.documentElement.dataset.stage === "ready", null, { timeout: 60_000 });
  await linkedPage.waitForFunction(() => document.querySelector("[data-film]").dataset.act === "8", null, { timeout: gpu ? 10_000 : 30_000 });
  await linked.close();
}

// The page's modules blocked: the hero settles on its own CSS a little
// after the entrance would have, and the boot's deadline hands the page
// back to the document.
async function checkBlockedModule(width, height, label) {
  const blocked = await browser.newContext({ viewport: { width, height } });
  const blockedPage = await blocked.newPage();
  await blockedPage.route(/\/_astro\/.*\.js$/, (route) => route.abort());
  await blockedPage.goto(`${base}/?film=${gpu ? "1" : "force"}`, { waitUntil: "commit" });
  await blockedPage.waitForFunction(() => document.documentElement.dataset.mode === "film", null, { timeout: 10_000 });
  await assertOnTop(blockedPage, ".site-nav .cta", `${label} blocked`);
  await blockedPage.waitForFunction(() => !document.documentElement.dataset.mode, null, { timeout: 15_000 });
  await blockedPage.waitForTimeout(300);
  const rested = await blockedPage.evaluate(() => ({
    title: getComputedStyle(document.querySelector("#act-1-title")).opacity,
    field: getComputedStyle(document.querySelector("[data-hero-field]")).visibility,
    picture: getComputedStyle(document.querySelector("[data-hero-device]")).opacity,
  }));
  assert.deepEqual(rested, { title: "1", field: "hidden", picture: "1" }, `${label}: a hero without its script settles on its own`);
  await assertOnTop(blockedPage, "#act-1 .actions .cta", `${label} blocked`);
  await blockedPage.screenshot({ path: path.join(output, `${label}-blocked.png`) });
  await blockedPage.locator("#act-4-title").scrollIntoViewIfNeeded();
  assert.ok(await blockedPage.locator("#act-4-title").isVisible(), `${label}: a later act is readable after a blocked film`);
  await blocked.close();
}

// The 3D slow, then failing: the entrance never waits for it. The picture
// turns the laptop in on schedule, the hero settles, and when the load
// fails the film gives the page to the document with the hero at rest.
async function checkFailedHardware(width, height, label) {
  const held = await browser.newContext({ viewport: { width, height } });
  const heldPage = await held.newPage();
  const errors = watchErrors(heldPage);
  let release;
  const released = new Promise((resolve) => { release = resolve; });
  await heldPage.route(/\/assets\/devices\/.*\.glb$/, async (route) => {
    await released;
    await route.abort();
  });
  await heldPage.goto(`${base}/?film=${gpu ? "1" : "force"}`, { waitUntil: "commit" });
  await heldPage.waitForFunction(() => window.BuildHero, null, { timeout: 15_000 });
  await waitForEntrance(heldPage, `${label} slow 3D`);
  assert.equal(await heldPage.evaluate(() => window.BuildHero.laptop.driver), "poster", `${label}: the picture turned the laptop in`);
  await heldPage.screenshot({ path: path.join(output, `${label}-slow-3d.png`) });
  release();
  await heldPage.waitForFunction(() => !document.documentElement.dataset.mode, null, { timeout: 15_000 });
  await heldPage.waitForTimeout(300);
  const picture = await heldPage.evaluate(() => getComputedStyle(document.querySelector("[data-hero-device]")).opacity);
  assert.equal(picture, "1", `${label}: the document keeps the hero's laptop after a failed load`);
  await assertOnTop(heldPage, "#act-1 .actions .cta", `${label} failed 3D`);
  assert.deepEqual(errors.filter((error) => !/glb|Failed to load resource/i.test(error)), [], `${label}: browser errors with failed 3D`);
  await held.close();
}

// --- the hero's entrance ---------------------------------------------------

// GSAP's clock slows on a loaded software renderer; wait for the entrance
// to say it is done, not for four seconds. Then nothing of it is left: no
// field, no attribute, no inline style on the copy, no running animation.
async function waitForEntrance(page, label, { scrolled = false } = {}) {
  await page.waitForFunction(() => window.BuildHero === null || window.BuildHero?.done, null, { timeout: gpu ? 15_000 : 120_000 });
  const rested = await page.evaluate(() => ({
    hero: document.documentElement.dataset.hero ?? null,
    field: document.querySelectorAll("[data-hero-field]").length,
    title: getComputedStyle(document.querySelector("#act-1-title")).opacity,
    // Nothing the entrance hid stays hidden (the film may write its own
    // visible styles on the copy as it re-measures).
    styled: [...document.querySelectorAll("#act-1 .act__copy, #act-1 .act__copy *")].filter((element) => /opacity: 0[;\s]|visibility: hidden/.test(`${element.getAttribute("style") || ""} `)).length,
    animations: document.getAnimations().filter((animation) => animation.playState === "running" && animation.effect?.target?.closest?.("#act-1")).length,
    active: window.BuildHero ? window.BuildHero.timeline.isActive() : false,
  }));
  // Scrolled into the film, the film's own scroll has taken the hero's copy.
  if (scrolled) Object.assign(rested, { title: "1", styled: 0 });
  assert.deepEqual(rested, { hero: null, field: 0, title: "1", styled: 0, animations: 0, active: false }, `${label}: the hero is still and the entrance let go of everything`);
}

// Scroll and resize listeners on the window, by the browser's own count.
async function windowListeners(page) {
  const session = await page.context().newCDPSession(page);
  const { result } = await session.send("Runtime.evaluate", { expression: "window" });
  const { listeners } = await session.send("DOMDebugger.getEventListeners", { objectId: result.objectId });
  await session.detach();
  return listeners.filter((listener) => ["scroll", "resize"].includes(listener.type)).length;
}

// Hold the entrance at a moment, as the scrubber does.
async function holdAt(page, time) {
  await page.evaluate((at) => {
    const hero = window.BuildHero;
    hero.hold();
    hero.timeline.pause();
    hero.timeline.time(at, false);
    window.BuildFilm?.sync?.();
  }, time);
  await page.waitForTimeout(150);
}

// How many pills are in the window below the bar, faint ones included.
function visiblePills(page) {
  return page.evaluate(() => {
    const below = document.querySelector(".site-nav").getBoundingClientRect().bottom;
    return [...document.querySelectorAll(".hero-pill")].filter((pill) => {
      const box = pill.getBoundingClientRect();
      return box.width > 0 && box.right > 0 && box.left < innerWidth && box.bottom > below && box.top < innerHeight;
    }).length;
  });
}

const ROWS = { review: "task-82", approval: "task-85", question: "task-86" };

// Held a hair before it lands, a request sits on its row's glow.
async function checkLanding(page, label, id, at) {
  await holdAt(page, at);
  const landed = await page.evaluate(({ attention, row }) => {
    const pill = document.querySelector(`[data-attention="${attention}"]`).getBoundingClientRect();
    const glow = document.querySelector(`[data-glow="${row}"]`).getBoundingClientRect();
    const [x, y] = [pill.x + pill.width / 2, pill.y + pill.height / 2];
    return { inside: x >= glow.left && x <= glow.right && y >= glow.top - 2 && y <= glow.bottom + 2, pill: [x, y], glow: [glow.left, glow.top, glow.right, glow.bottom] };
  }, { attention: id, row: ROWS[id] });
  assert.ok(landed.inside, `${label}: ${id} lands on its row (${JSON.stringify(landed)})`);
}

// The entrance phase by phase, held on its own clock: a full field on the
// first frame, the ripple, the laptop, each request landing on its row, the
// copy, stillness. The bar's download takes the pointer throughout.
async function checkEntrancePhases(page, label, { narrow }) {
  const [low, high] = narrow ? [24, 36] : [50, 80];
  const count = await visiblePills(page);
  assert.ok(count >= low && count <= high, `${label}: ${count} pills in the first frame`);
  const timing = await page.evaluate(() => window.BuildHero.timing);
  const shots = [
    ["1-field", 0.6],
    ["2-ripple", (timing.ripple[0] + timing.ripple[1]) / 2],
    ["3-laptop", timing.converge[0] - 0.05],
    ["4-landing", timing.landings[1]],
    ["5-message", timing.message[1] - 0.15],
    ["6-quiet", timing.settle[1] - 0.01],
  ];
  for (const [name, at] of shots) {
    await holdAt(page, at);
    await assertOnTop(page, ".site-nav .cta", `${label} ${name}`);
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - innerWidth);
    assert.ok(overflow <= 1, `${label} ${name}: no horizontal overflow (${overflow})`);
    await page.screenshot({ path: path.join(output, `${label}-${name}.png`) });
  }
  for (const [index, id] of ["review", "approval", "question"].entries()) {
    await checkLanding(page, label, id, timing.landings[index] - 0.02);
  }
}

async function openEntrance(width, height) {
  const context = await browser.newContext({ viewport: { width, height } });
  const page = await context.newPage();
  const errors = watchErrors(page);
  await page.goto(`${base}/?film=${gpu ? "1" : "force"}`, { waitUntil: "load" });
  await page.waitForFunction(() => window.BuildHero !== undefined, null, { timeout: 30_000 });
  assert.ok(await page.evaluate(() => Boolean(window.BuildHero)), "the entrance plays on a first visit");
  return { context, page, errors };
}

// The whole entrance at a size, then the same tab again.
async function checkFullEntrance(width, height, label, narrow) {
  const { context, page, errors } = await openEntrance(width, height);
  const during = await windowListeners(page);
  await checkEntrancePhases(page, label, { narrow });
  await page.evaluate(() => window.BuildHero.finish());
  await waitForEntrance(page, label);
  assert.equal(await windowListeners(page), during - 2, `${label}: the entrance's scroll and resize listeners are gone`);
  await checkHeroPointerPath(page, label);
  await page.reload({ waitUntil: "commit" });
  await page.waitForSelector("#act-1-title", { state: "attached" });
  const repeat = await page.evaluate(() => ({ hero: document.documentElement.dataset.hero ?? null, title: getComputedStyle(document.querySelector("#act-1-title")).opacity }));
  assert.deepEqual(repeat, { hero: null, title: "1" }, `${label}: a repeat visit shows the hero at rest`);
  assert.deepEqual(errors, [], `${label}: browser errors`);
  await context.close();
}

// Scrolling away mid-entrance finishes it on the spot and keeps the scroll.
async function checkScrollAway(width, height, label) {
  const { context, page, errors } = await openEntrance(width, height);
  await page.waitForFunction(() => window.BuildHero.timeline.time() > 1.3, null, { timeout: 60_000 });
  await page.mouse.move(width / 2, height / 2);
  await page.mouse.wheel(0, 400);
  await page.waitForFunction(() => window.BuildHero.done, null, { timeout: 10_000 });
  assert.equal(await page.evaluate(() => window.BuildHero.reason), "scroll");
  await page.waitForTimeout(300);
  assert.ok(await page.evaluate(() => scrollY) > 0, `${label}: the scroll is the visitor's`);
  await waitForEntrance(page, `${label} scrolled`, { scrolled: true });
  assert.deepEqual(errors, [], `${label}: browser errors after scrolling away`);
  await context.close();
}

// A hidden tab holds the entrance; a change of width (a turned phone, a
// resized window) finishes it.
async function checkPauseAndResize(width, height, label, narrow) {
  const { context, page, errors } = await openEntrance(width, height);
  const paused = await page.evaluate(() => {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
    document.dispatchEvent(new Event("visibilitychange"));
    const held = window.BuildHero.timeline.paused();
    Object.defineProperty(document, "hidden", { configurable: true, get: () => false });
    document.dispatchEvent(new Event("visibilitychange"));
    return held && !window.BuildHero.timeline.paused();
  });
  assert.ok(paused, `${label}: a hidden tab pauses the entrance and a shown one resumes it`);
  await page.setViewportSize(narrow ? { width: height, height: width } : { width: Math.round(width * 0.8), height });
  await page.waitForFunction(() => window.BuildHero.done, null, { timeout: 10_000 });
  assert.equal(await page.evaluate(() => window.BuildHero.reason), "resize");
  await waitForEntrance(page, `${label} resized`);
  assert.deepEqual(errors, [], `${label}: browser errors after a resize`);
  await context.close();
}

async function inspectEntrance(width, height) {
  const narrow = width < 768;
  const label = `${width}x${height}-entrance`;
  await checkFullEntrance(width, height, label, narrow);
  await checkScrollAway(width, height, label);
  await checkPauseAndResize(width, height, label, narrow);
  findings.push({ label, viewport: [width, height] });
}

// The bar: on screen through the film, the story laid out below it, and its
// download takes the pointer wherever the visitor is.
async function inspectNav(width, height) {
  const label = `${width}x${height}-nav`;
  const { context, page, errors, state } = await openFilm(width, height, label);
  const nav = page.locator(".site-nav");
  assert.ok(await nav.isVisible(), `${label}: the bar is there on the hero`);
  const before = await page.locator("#act-4-title").evaluate(() => document.querySelector(".act[data-act='4'] .act__copy").getBoundingClientRect().top);
  await rest(page, 4);
  await page.waitForTimeout(500);
  const after = await page.locator("#act-4-title").evaluate(() => document.querySelector(".act[data-act='4'] .act__copy").getBoundingClientRect().top);
  assert.equal(before, after, `${label}: the story sits in the same place`);
  assert.ok(await nav.isVisible(), `${label}: the bar stays in act 4`);
  await assertOnTop(page, ".site-nav .cta", `${label} act 4`);
  await page.screenshot({ path: path.join(output, `${label}-act-4.png`) });
  assert.deepEqual(errors, [], `${label}: browser errors`);
  findings.push({ label, viewport: [width, height], mode: state.mode });
  await context.close();
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
  // The re-measured pin keeps the visitor in the act they were reading.
  await page.waitForFunction(() => document.querySelector("[data-film]").dataset.act === "7", null, { timeout: gpu ? 5000 : 60_000 })
    .catch(() => {});
  assert.equal(await page.evaluate(() => document.querySelector("[data-film]").dataset.act), "7", `${label}: a resize keeps the film in act 7`);
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
  for (const [width, height] of [[1440, 900], [390, 844]]) await inspectEntrance(width, height);
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
