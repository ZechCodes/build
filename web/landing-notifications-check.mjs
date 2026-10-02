// Rendered coverage, spacing and mask checks. Start scripts/preview-landing.py,
// then CHROMIUM_PATH=/usr/bin/chromium node web/landing-notifications-check.mjs.
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { chromium } from "playwright";

const sizes = [[390, 667], [390, 844], [768, 1024], [1280, 900], [1920, 1080], [2560, 1440]];
const base = process.env.LANDING_URL || "http://127.0.0.1:4173";
const output = process.env.LANDING_NOTIFICATION_DIR || "/tmp/build-landing-notifications";
assert.ok(["127.0.0.1", "localhost", "[::1]"].includes(new URL(base).hostname), "Use a local landing preview");
await fs.mkdir(output, { recursive: true });

// Actual boxes, including the ripple and request flights, at a held instant.
// Work inside the visible field; offscreen cards do not crowd the composition.
function measureField() {
  const field = document.querySelector("[data-hero-field]");
  const bounds = field.getBoundingClientRect();
  const top = Math.max(bounds.top, document.querySelector(".site-nav").getBoundingClientRect().bottom);
  const bottom = Math.min(bounds.bottom, innerHeight);
  const cards = [...field.querySelectorAll(".hero-pill")].filter(pill => {
    const style = getComputedStyle(pill);
    return Number(style.opacity) > 0.05 && style.visibility !== "hidden";
  }).map(pill => ({ ...pill.getBoundingClientRect().toJSON(), key: pill.dataset.pill, attention: pill.dataset.attention }))
    .filter(box => box.width && box.height && box.right > 0 && box.left < innerWidth && box.bottom > top && box.top < bottom);
  let nearest = { gap: Infinity };
  for (let first = 0; first < cards.length; first += 1) {
    for (let second = first + 1; second < cards.length; second += 1) {
      const [a, b] = [cards[first], cards[second]];
      const dx = Math.max(0, a.left - b.right, b.left - a.right);
      const dy = Math.max(0, a.top - b.bottom, b.top - a.bottom);
      const gap = Math.hypot(dx, dy);
      if (gap < nearest.gap) nearest = { gap, first: a.key, second: b.key, attention: a.attention || b.attention || null };
    }
  }
  const gaps = [0.2, 0.5, 0.8].map(share => {
    const x = bounds.width * share;
    const start = top + (bottom - top) * 0.18;
    const end = bottom - (bottom - top) * 0.18;
    const intervals = cards.filter(box => box.left <= x + bounds.width * 0.05 && box.right >= x - bounds.width * 0.05 && box.bottom >= start && box.top <= end)
      .sort((a, b) => a.top - b.top);
    let covered = start;
    let gap = 0;
    for (const box of intervals) {
      gap = Math.max(gap, box.top - covered);
      covered = Math.max(covered, box.bottom);
    }
    return Math.max(gap, end - covered);
  });
  return { gaps, nearest, overflow: document.documentElement.scrollWidth - innerWidth, count: cards.length };
}

async function holdAt(page, time) {
  await page.evaluate(at => {
    window.BuildHero.hold();
    window.BuildHero.timeline.pause();
    window.BuildHero.timeline.time(at, false);
  }, time);
}

// Paint white through the real field mask over black, without changing its
// geometry or mask styles. Decode the resulting PNG in Chromium's canvas:
// its red channel is the rendered mask alpha. No PNG parser dependency.
async function sampleMask(page, label) {
  const region = await page.locator("[data-hero-field]").evaluate(field => {
    const box = field.getBoundingClientRect();
    return {
      left: box.left, right: box.right - 1,
      top: Math.max(box.top, document.querySelector(".site-nav").getBoundingClientRect().bottom),
      bottom: Math.min(box.bottom, innerHeight) - 1,
      sampleBottom: innerWidth < 768 || box.bottom <= innerHeight + 1,
    };
  });
  const paint = await page.addStyleTag({ content: `
    html, body { background: #000 !important; }
    body { visibility: hidden !important; }
    [data-hero-field] { visibility: visible !important; background: #fff !important; opacity: 1 !important; }
    [data-hero-field] * { visibility: hidden !important; }
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
    const { left, right, top, bottom, sampleBottom } = region;
    const [width, height] = [right - left, bottom - top];
    const depths = [0, 0.01, 0.03, 0.06, 0.1, 0.18];
    const edges = {
      left: depths.map(depth => alpha(left + width * depth, top + height / 2)),
      right: depths.map(depth => alpha(right - width * depth, top + height / 2)),
      top: depths.map(depth => alpha(left + width / 2, top + height * depth)),
    };
    // A stacked desktop document hero can continue below the viewport.
    // Film profiles and every phone must fade at the viewport bottom.
    if (sampleBottom) edges.bottom = depths.map(depth => alpha(left + width / 2, bottom - height * depth));
    return { depths, edges, center: alpha(left + width / 2, top + height / 2) };
  }, { png: shot.toString("base64"), region });
}

function maskFailures(mask, label) {
  const failures = [];
  for (const [edge, values] of Object.entries(mask.edges)) {
    if (values[0] > 0.02) failures.push(`${label}: ${edge} boundary alpha ${values[0].toFixed(3)}`);
    if (values[2] < 0.08 || values[2] > 0.35) failures.push(`${label}: ${edge} alpha at 3% is ${values[2].toFixed(3)} (expected 0.08–0.35)`);
    if (values[3] < 0.22 || values[3] > 0.65) failures.push(`${label}: ${edge} alpha at 6% is ${values[3].toFixed(3)} (expected 0.22–0.65)`);
    if (values[4] < 0.4 || values[4] > 0.9) failures.push(`${label}: ${edge} alpha at 10% is ${values[4].toFixed(3)} (expected 0.4–0.9)`);
    if (values[5] < 0.95) failures.push(`${label}: ${edge} still fades beyond 18% (${values[5].toFixed(3)})`);
    if (values.some((value, index) => index && value < values[index - 1] - 0.02)) failures.push(`${label}: ${edge} alpha must increase inward`);
  }
  if (mask.center < 0.98) failures.push(`${label}: the center must stay full strength`);
  return failures;
}

async function checkField(browser, width, height, mode) {
  const label = `${width}x${height}-${mode}`;
  const context = await browser.newContext({ viewport: { width, height } });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.goto(`${base}/?hero=play&film=${mode === "film" ? "force" : "0"}`, { waitUntil: "load" });
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
  const masks = [];
  const failures = [];
  for (const time of [0.05, timing.field[1] - 0.01]) {
    await holdAt(page, time);
    const mask = await sampleMask(page, `${label}-${time.toFixed(2)}s`);
    masks.push({ time, ...mask });
    failures.push(...maskFailures(mask, `${label} at ${time.toFixed(2)}s`));
  }
  const frames = [];
  const flights = timing.landings.flatMap(at => [at - timing.flight, at - timing.flight / 2, at - 0.04]);
  const times = [...new Set([0.05, 0.2, 0.4, 0.6, 0.8, timing.field[1] - 0.01, ...Array.from({ length: 8 }, (_, index) => timing.ripple[0] + index * 0.1), ...flights])];
  for (const time of times.sort((a, b) => a - b)) {
    await holdAt(page, time);
    const state = await page.evaluate(measureField);
    frames.push({ time, ...state });
    if (state.nearest.gap < 4 - 0.05) failures.push(`${label} at ${time.toFixed(2)}s: cards need a 4px gap: ${JSON.stringify(state.nearest)}`);
    if (time < timing.field[1] && Math.max(...state.gaps) > 42) failures.push(`${label} at ${time.toFixed(2)}s: empty bands ${state.gaps.map(gap => gap.toFixed(1))}px`);
    if (state.overflow > 1) failures.push(`${label}: horizontal scroll ${state.overflow}px`);
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
  console.log(`Notifications: ${label} passed (rendered mask alpha, spacing, coverage, overflow, cleanup)`);
}

async function checkLabField(browser) {
  const context = await browser.newContext({ viewport: { width: 390, height: 667 }, javaScriptEnabled: false });
  const page = await context.newPage();
  await page.goto(`${base}/lab/notifications-133c027df9b5`, { waitUntil: "load" });
  const stage = await page.locator("[data-lab-stage]").boundingBox();
  assert.deepEqual(await page.locator("[data-hero-field]").boundingBox(), stage, "short-phone cap leaves the lab field full height");
  await context.close();
  console.log("Notifications: short-phone lab retains its full field");
}

const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROMIUM_PATH, args: ["--enable-unsafe-swiftshader"] });
try {
  await checkLabField(browser);
  for (const [width, height] of sizes) {
    await checkField(browser, width, height, width < 768 ? "document" : "film");
    if (width >= 768) await checkField(browser, width, height, "document");
  }
} finally {
  await browser.close();
}
