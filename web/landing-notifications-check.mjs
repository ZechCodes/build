// Rendered density, motion, mask and hand-off checks for the home hero. Start scripts/preview-landing.py,
// then CHROMIUM_PATH=/usr/bin/chromium node web/landing-notifications-check.mjs.
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { chromium } from "playwright";
import { REQUEST_SELECT } from "../landing/src/hero/wall.js";

const sizes = [[390, 667], [390, 844], [768, 1024], [1280, 900], [1920, 1080], [2560, 1440]];
const base = process.env.LANDING_URL || "http://127.0.0.1:4173";
const heroPath = "/";
const output = process.env.LANDING_NOTIFICATION_DIR || "/tmp/build-landing-notifications";
assert.ok(["127.0.0.1", "localhost", "[::1]"].includes(new URL(base).hostname), "Use a local landing preview");
await fs.mkdir(output, { recursive: true });

// Stable element indices let us follow each routine note through its slide,
// still hold, and fade while the preview timeline is held at sample times.
function measureField() {
  const field = document.querySelector("[data-hero-field]");
  const routine = field.querySelector("[data-wall-routine]");
  const bounds = field.getBoundingClientRect();
  const cards = [...routine.querySelectorAll(".wall-note[data-note]")].map((card, index) => {
    const box = card.getBoundingClientRect();
    return {
      index, key: card.dataset.note, opacity: Number(getComputedStyle(card).opacity),
      x: box.left + box.width / 2, y: box.top + box.height / 2,
      width: box.width, height: box.height,
    };
  });
  const interior = cards.filter(card => card.opacity >= 0.65 && card.width > 0 && card.height > 0
    && card.x + card.width / 2 >= bounds.left + bounds.width * 0.1
    && card.x - card.width / 2 <= bounds.right - bounds.width * 0.1
    && card.y + card.height / 2 >= bounds.top + bounds.height * 0.1
    && card.y - card.height / 2 <= bounds.bottom - bounds.height * 0.1);
  const quadrants = [0, 0, 0, 0];
  for (const card of interior) quadrants[(card.x >= bounds.left + bounds.width / 2 ? 1 : 0) + (card.y >= bounds.top + bounds.height / 2 ? 2 : 0)] += 1;
  return {
    field: bounds.toJSON(), routine: routine.getBoundingClientRect().toJSON(),
    cards, count: interior.length, quadrants,
    overflow: document.documentElement.scrollWidth - innerWidth,
  };
}

async function holdAt(page, time) {
  await page.evaluate(at => {
    window.BuildHero.hold();
    window.BuildHero.timeline.pause();
    window.BuildHero.timeline.time(at, false);
    window.BuildFilm?.sync?.();
  }, time);
  await page.waitForTimeout(80);
}

// Paint white through the routine layer's real mask over black. The requests
// live outside this layer so their flights cannot be clipped by its edges.
async function sampleMask(page, label) {
  const region = await page.locator("[data-wall-routine]").evaluate(layer => layer.getBoundingClientRect().toJSON());
  const paint = await page.addStyleTag({ content: `
    html, body { background: #000 !important; }
    body { visibility: hidden !important; }
    [data-hero-field], [data-wall-routine] { visibility: visible !important; opacity: 1 !important; }
    [data-wall-routine] { background: #fff !important; }
    [data-wall-routine] * { visibility: hidden !important; }
  ` });
  let shot;
  try {
    shot = await page.screenshot({ path: path.join(output, `${label}-mask.png`) });
  } finally {
    await paint.evaluate(element => element.remove());
  }
  return page.evaluate(async ({ png, region }) => {
    const image = new Image();
    image.src = `data:image/png;base64,${png}`;
    await image.decode();
    const canvas = document.createElement("canvas");
    canvas.width = image.width;
    canvas.height = image.height;
    const context = canvas.getContext("2d");
    context.drawImage(image, 0, 0);
    const alpha = (x, y) => context.getImageData(Math.round(x), Math.round(y), 1, 1).data[0] / 255;
    const { left, right, top, bottom, width, height } = region;
    const depths = [0.005, 0.03, 0.06, 0.1, 0.18];
    const edges = {
      left: depths.map(depth => alpha(left + width * depth, top + height / 2)),
      right: depths.map(depth => alpha(right - width * depth, top + height / 2)),
      top: depths.map(depth => alpha(left + width / 2, top + height * depth)),
      bottom: depths.map(depth => alpha(left + width / 2, bottom - height * depth)),
    };
    return { depths, edges, center: alpha(left + width / 2, top + height / 2) };
  }, { png: shot.toString("base64"), region });
}

function maskFailures(mask, label) {
  const failures = [];
  for (const [edge, values] of Object.entries(mask.edges)) {
    if (values[0] > 0.16) failures.push(`${label}: ${edge} begins at ${values[0].toFixed(2)} alpha`);
    if (values[2] <= values[0] + 0.12) failures.push(`${label}: ${edge} has no visible inward fade`);
    if (values[4] < 0.88) failures.push(`${label}: ${edge} still faded at 18% (${values[4].toFixed(2)})`);
    if (values.some((value, index) => index && value < values[index - 1] - 0.04)) failures.push(`${label}: ${edge} fades backwards`);
  }
  if (mask.center < 0.97) failures.push(`${label}: the center must stay full strength`);
  return failures;
}

function frameFailures(frame, label, height, checkDensity) {
  const failures = [];
  const { field, routine, count, quadrants, overflow } = frame;
  if (Math.abs(field.left - routine.left) > 1 || Math.abs(field.top - routine.top) > 1
    || Math.abs(field.width - routine.width) > 1 || Math.abs(field.height - routine.height) > 1) {
    failures.push(`${label}: routine layer does not cover the field`);
  }
  if (field.bottom > height + 2 || field.top < -2) failures.push(`${label}: field exceeds the visible viewport`);
  // Count cards that intersect the central rectangle: a card whose center
  // sits under the edge mask can still show most of its width on a phone.
  if (checkDensity) {
    const minimum = Math.min(60, Math.max(6, Math.floor(field.width * field.height / 48_000)));
    if (count < minimum) failures.push(`${label}: only ${count} interior cards (expected at least ${minimum})`);
    if (quadrants.some(value => value < (field.width < 768 ? 1 : 3))) failures.push(`${label}: sparse quadrant counts ${quadrants.join("/")}`);
  }
  if (overflow > 1) failures.push(`${label}: ${overflow}px horizontal overflow`);
  return failures;
}

function motionFailures(frames, label) {
  const failures = [];
  const perCard = new Map();
  for (const frame of frames) for (const card of frame.cards) {
    if (!perCard.has(card.index)) perCard.set(card.index, []);
    perCard.get(card.index).push({ time: frame.time, ...card });
  }
  const first = frames[0].field;
  const entered = [0, 0, 0, 0];
  const left = [0, 0, 0, 0];
  let held = 0;
  for (const observations of perCard.values()) {
    const visible = observations.filter(card => card.opacity > 0.15);
    if (!visible.length) continue;
    const spread = Math.hypot(
      Math.max(...visible.map(card => card.x)) - Math.min(...visible.map(card => card.x)),
      Math.max(...visible.map(card => card.y)) - Math.min(...visible.map(card => card.y)),
    );
    if (spread > 64) failures.push(`${label}: ${observations[0].key} travels ${spread.toFixed(1)}px within its slot`);
    const steady = observations.some((card, index) => {
      const next = observations[index + 1];
      return next && card.opacity > 0.78 && next.opacity > 0.78 && next.time - card.time >= 0.09
        && Math.hypot(next.x - card.x, next.y - card.y) < 1.5;
    });
    if (steady) held += 1;
    const peak = Math.max(...observations.map(card => card.opacity));
    if (peak < 0.7) continue;
    const at = observations.find(card => card.opacity === peak);
    const quadrant = (at.x >= first.left + first.width / 2 ? 1 : 0) + (at.y >= first.top + first.height / 2 ? 2 : 0);
    const firstHigh = observations.findIndex(card => card.opacity > 0.6);
    if (firstHigh > 0 && observations.slice(0, firstHigh).some(card => card.opacity < 0.3)) entered[quadrant] += 1;
    if (firstHigh >= 0 && observations.slice(firstHigh + 1).some(card => card.opacity < 0.3)) left[quadrant] += 1;
  }
  if (held < Math.max(8, perCard.size * 0.08)) failures.push(`${label}: too few cards hold still (${held}/${perCard.size})`);
  if (entered.some(value => value < 2) || left.some(value => value < 2)) failures.push(`${label}: repeated replacements miss a quadrant (in ${entered}, out ${left})`);
  return failures;
}

async function entryDirectionFailures(page, label) {
  const inspected = await page.locator("[data-wall-routine]").evaluate(routine => {
    const cards = [...routine.querySelectorAll(".wall-note[data-note]")];
    const entries = cards.map(card => {
      const animation = card.getAnimations().find(item => item.effect?.getKeyframes);
      const frames = animation?.effect.getKeyframes() || [];
      return frames.map(frame => ({
        x: Number(frame.transform?.match(/^translate3d\(([-\d.]+)px,/)?.[1]),
        opacity: Number(frame.opacity),
      })).filter(frame => Number.isFinite(frame.x));
    });
    return { total: cards.length, entries };
  });
  const failures = [];
  const arrivals = inspected.entries.filter(frames => frames.some(frame => frame.x >= 50));
  if (arrivals.length < inspected.total * 0.25) failures.push(`${label}: too few rendered cards have a visible right-side start (${arrivals.length}/${inspected.total})`);
  for (const frames of arrivals) {
    if (frames.some(frame => frame.x < -1)) failures.push(`${label}: a routine card enters from the left`);
    const start = frames.findIndex(frame => frame.x >= 50);
    if (!frames.slice(start + 1).some(frame => Math.abs(frame.x) <= 1 && frame.opacity >= 0.65)) {
      failures.push(`${label}: a right-side arrival never settles in its slot`);
    }
  }
  return [...new Set(failures)];
}

const ROWS = { review: "task-82", approval: "task-85", question: "task-86" };

async function requestFailures(page, label, timing) {
  const failures = [];
  const beforeSelection = timing.field[1] - 0.9;
  await holdAt(page, beforeSelection);
  const baseline = await page.evaluate(() => {
    window.__wallSelectedCards = Object.fromEntries(["review", "approval", "question"].map(id =>
      [id, document.querySelector(`.wall-request[data-attention="${id}"]`)]));
    return Object.fromEntries(Object.entries(window.__wallSelectedCards).map(([id, card]) => {
      const box = card.getBoundingClientRect();
      return [id, {
        key: card.dataset.note, text: card.querySelector(":scope > .wall-note__text").textContent,
        opacity: Number(getComputedStyle(card).opacity),
        mint: Number(getComputedStyle(card.querySelector(".wall-note--mint")).opacity),
        inRoutine: Boolean(card.closest("[data-wall-routine]")),
        x: box.left + box.width / 2, y: box.top + box.height / 2,
        width: box.width, height: box.height,
      }];
    }));
  });
  for (const [id, card] of Object.entries(baseline)) {
    if (!card.key || !card.text || card.opacity < 0.8 || card.mint > 0.05 || !card.inRoutine) {
      failures.push(`${label}: ${id} is not an ordinary visible wall card before selection (${JSON.stringify(card)})`);
    }
  }
  const phases = Object.keys(ROWS).flatMap((id, index) => {
    const landing = timing.landings[index];
    const takeOff = landing - timing.flight;
    return [
      { id, phase: "neutral", time: REQUEST_SELECT[index] - 0.04 },
      { id, phase: "mint", time: REQUEST_SELECT[index] + 0.3 },
      { id, phase: "flight", time: takeOff + 0.15 },
      { id, phase: "row", time: landing - 0.02 },
      { id, phase: "gone", time: landing + 0.04 },
    ];
  }).sort((a, b) => a.time - b.time);
  for (const { id, phase, time } of phases) {
    await holdAt(page, time);
    const state = await page.evaluate(({ attention, rowId }) => {
      const request = document.querySelector(`.wall-request[data-attention="${attention}"]`);
      const green = request.querySelector(".wall-note--mint");
      const glow = document.querySelector(`[data-glow="${rowId}"]`);
      const pill = request.getBoundingClientRect();
      const target = glow.getBoundingClientRect();
      const x = pill.left + pill.width / 2;
      const y = pill.top + pill.height / 2;
      return {
        same: request === window.__wallSelectedCards[attention],
        key: request.dataset.note,
        text: request.querySelector(":scope > .wall-note__text").textContent,
        opacity: Number(getComputedStyle(request).opacity),
        mint: Number(getComputedStyle(green).opacity),
        inRoutine: Boolean(request.closest("[data-wall-routine]")),
        inside: x >= target.left - 4 && x <= target.right + 4 && y >= target.top - 4 && y <= target.bottom + 4,
        x, y, width: pill.width, height: pill.height, target: target.toJSON(),
      };
    }, { attention: id, rowId: ROWS[id] });
    const first = baseline[id];
    if (!state.same || state.key !== first.key || state.text !== first.text) failures.push(`${label}: ${id} was replaced instead of selected (${JSON.stringify(state)})`);
    if (phase === "neutral" || phase === "mint") {
      const shift = Math.hypot(state.x - first.x, state.y - first.y);
      if (shift > 2 || Math.abs(state.width - first.width) > 2 || Math.abs(state.height - first.height) > 2) {
        failures.push(`${label}: ${id} pops or moves before flight (${JSON.stringify(state)})`);
      }
      if (!state.inRoutine || state.opacity < 0.8) failures.push(`${label}: ${id} leaves the wall before its flight (${JSON.stringify(state)})`);
    }
    if (phase === "neutral" && state.mint > 0.05) failures.push(`${label}: ${id} is mint before selection (${JSON.stringify(state)})`);
    if (phase === "mint" && state.mint < 0.95) failures.push(`${label}: ${id} does not turn visibly mint (${JSON.stringify(state)})`);
    if (phase === "flight" && state.inRoutine) failures.push(`${label}: ${id} remains under the edge mask during flight`);
    if (phase === "row" && !state.inside) failures.push(`${label}: ${id} misses its Needs you row (${JSON.stringify(state)})`);
    if (phase === "gone" && state.opacity > 0.06) failures.push(`${label}: ${id} remains visible after landing (${state.opacity})`);
  }
  await holdAt(page, beforeSelection);
  const rewound = await page.locator('.wall-request[data-attention="review"]').evaluate(request => ({
    same: request === window.__wallSelectedCards.review,
    opacity: Number(getComputedStyle(request).opacity),
    mint: Number(getComputedStyle(request.querySelector(".wall-note--mint")).opacity),
    inRoutine: Boolean(request.closest("[data-wall-routine]")),
    visibility: getComputedStyle(request).visibility,
  }));
  if (!rewound.same || rewound.opacity < 0.8 || rewound.mint > 0.05 || !rewound.inRoutine || rewound.visibility !== "visible") {
    failures.push(`${label}: backward scrub does not restore the neutral review request (${JSON.stringify(rewound)})`);
  }
  return failures;
}

async function checkField(browser, width, height, mode) {
  const label = `${width}x${height}-${mode}`;
  const context = await browser.newContext({ viewport: { width, height } });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.goto(`${base}${heroPath}?hero=play&film=${mode === "film" ? "force" : "0"}`, { waitUntil: "load" });
  await page.waitForFunction(() => window.BuildHero !== undefined);
  assert.ok(await page.evaluate(() => Boolean(window.BuildHero)), `${label}: opening runs`);
  await holdAt(page, 0.6);
  if (mode === "film") {
    await page.waitForFunction(() => document.documentElement.dataset.stage === "ready", null, { timeout: 60_000 });
  }
  const currentMode = () => page.evaluate(() => document.documentElement.dataset.mode || "document");
  assert.equal(await currentMode(), mode, `${label}: requested mode starts`);
  const boxBefore = await page.locator("#act-1").boundingBox();
  const timing = await page.evaluate(() => window.BuildHero.timing);
  assert.equal(timing.field[1], 3, `${label}: the wall holds a 3s field beat`);
  assert.ok(Math.abs(timing.laptop[1] - timing.laptop[0] - 2.325) < 0.001, `${label}: the laptop has a 2.325s reveal`);
  assert.ok(Math.abs(timing.settle[1] - 6.425) < 0.001, `${label}: the full entrance ends near 6.425s`);
  const masks = [];
  const failures = [];
  for (const time of [0.05, timing.field[1] - 0.05]) {
    await holdAt(page, time);
    const mask = await sampleMask(page, `${label}-${time.toFixed(2)}s`);
    masks.push({ time, ...mask });
    failures.push(...maskFailures(mask, `${label} at ${time.toFixed(2)}s`));
  }
  const frames = [];
  const times = Array.from({ length: 21 }, (_, index) => 0.05 + index * (timing.field[1] - 0.1) / 20);
  for (const time of times) {
    await holdAt(page, time);
    const state = await page.evaluate(measureField);
    frames.push({ time, ...state });
    failures.push(...frameFailures(state, `${label} at ${time.toFixed(2)}s`, height, time <= timing.field[1] - 0.6));
  }
  failures.push(...motionFailures(frames, label));
  failures.push(...await entryDirectionFailures(page, label));
  failures.push(...await requestFailures(page, label, timing));
  for (const [name, time] of [["field", 0.7], ["ripple", (timing.ripple[0] + timing.ripple[1]) / 2], ["mint", timing.landings[1] - timing.flight / 2]]) {
    await holdAt(page, time);
    await page.screenshot({ path: path.join(output, `${label}-${name}.png`) });
  }
  await fs.writeFile(path.join(output, `${label}.json`), JSON.stringify({ masks, frames, failures }, null, 2));
  assert.deepEqual(failures, [], `${label}: field composition`);
  assert.equal(await currentMode(), mode, `${label}: requested mode remains active`);
  await page.evaluate(() => window.BuildHero.finish());
  assert.deepEqual(await page.locator("#act-1").boundingBox(), boxBefore, `${label}: no hero layout shift`);
  assert.equal(await page.locator("[data-hero-field]").count(), 0, `${label}: opening clears before copy`);
  const clickable = await page.locator("#act-1 .cta").evaluate(element => {
    const box = element.getBoundingClientRect();
    return element.contains(document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2));
  });
  assert.ok(clickable, `${label}: the CTA is under the pointer`);
  assert.deepEqual(errors, [], `${label}: no browser errors`);
  await context.close();
  console.log(`Notifications: ${label} passed (density, four-edge fade, still holds, replacements, mint handoff, cleanup)`);
}

async function checkHeightResize(browser) {
  const context = await browser.newContext({ viewport: { width: 390, height: 844 } });
  try {
    const page = await context.newPage();
    await page.goto(`${base}${heroPath}?hero=play&film=0`, { waitUntil: "load" });
    await page.waitForFunction(() => window.BuildHero !== undefined);
    assert.ok(await page.evaluate(() => Boolean(window.BuildHero)), "resize: opening runs");
    await page.setViewportSize({ width: 390, height: 667 });
    await page.waitForFunction(() => window.BuildHero.done);
    assert.equal(await page.evaluate(() => window.BuildHero.reason), "resize", "height-only resize settles the entrance");
    assert.equal(await page.locator("[data-hero-field]").count(), 0, "height-only resize removes the old field geometry");
    console.log("Notifications: height-only resize settles the wall");
  } finally {
    await context.close();
  }
}

const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROMIUM_PATH, args: ["--enable-unsafe-swiftshader"] });
try {
  for (const [width, height] of sizes) {
    await checkField(browser, width, height, width < 768 ? "document" : "film");
    if (width >= 768) await checkField(browser, width, height, "document");
  }
  await checkHeightResize(browser);
} finally {
  await browser.close();
}
