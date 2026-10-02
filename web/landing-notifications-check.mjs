// Geometry checks complement visual review of the opening notification field.
// Run against scripts/preview-landing.py only; no account or network service is used.
import assert from "node:assert/strict";
import { chromium } from "playwright";

const sizes = [[390, 844], [768, 1024], [1280, 900], [1920, 1080], [2560, 1440]];
const base = process.env.LANDING_URL || "http://127.0.0.1:4173";
assert.ok(["127.0.0.1", "localhost", "[::1]"].includes(new URL(base).hostname), "Use a local landing preview");

// The longest empty vertical stretch in each of three interior column regions.
// Counting cards alone misses a field clumped into bands or one corner.
function measureCoverage() {
  const field = document.querySelector("[data-hero-field]");
  const bounds = field.getBoundingClientRect();
  const pills = [...field.querySelectorAll(".hero-pill")]
    .filter(pill => Number(getComputedStyle(pill).opacity) > 0.1)
    .map(pill => pill.getBoundingClientRect())
    .filter(box => box.width && box.height);
  const top = Math.max(bounds.top, 56) + bounds.height * 0.12;
  const bottom = Math.min(bounds.bottom, innerHeight) - bounds.height * 0.12;
  const gaps = [0.2, 0.5, 0.8].map(share => {
    const x = bounds.left + bounds.width * share;
    const intervals = pills.filter(box => box.left <= x + bounds.width * 0.05 && box.right >= x - bounds.width * 0.05 && box.bottom >= top && box.top <= bottom)
      .sort((a, b) => a.top - b.top);
    let end = top;
    let gap = 0;
    for (const box of intervals) {
      gap = Math.max(gap, box.top - end);
      end = Math.max(end, box.bottom);
    }
    return Math.max(gap, bottom - end);
  });
  const shortTracks = [...field.querySelectorAll(".hero-lane")].filter(lane => {
    const box = lane.getBoundingClientRect();
    if (!box.height || box.bottom < top || box.top > bottom) return false;
    const cards = [...lane.querySelectorAll(".hero-pill")].map(pill => pill.getBoundingClientRect());
    return Math.min(...cards.map(card => card.left)) > bounds.left + 40 ||
      Math.max(...cards.map(card => card.right)) < bounds.right - 40;
  }).map(lane => lane.dataset.lane);
  return { gaps, shortTracks, mask: getComputedStyle(field).maskImage, overflow: document.documentElement.scrollWidth - innerWidth };
}

async function checkField(browser, width, height) {
  const context = await browser.newContext({viewport: {width, height}});
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", error => errors.push(error.message));
  // The document uses the same field and lets these layout checks run without
  // a GPU. The full landing gate separately exercises film startup and flight.
  await page.goto(`${base}/?hero=play&film=0`, {waitUntil: "load"});
  await page.waitForFunction(() => window.BuildHero !== undefined);
  assert.ok(await page.evaluate(() => Boolean(window.BuildHero)), `${width}: opening runs`);
  const boxBefore = await page.locator("#act-1").boundingBox();
  for (const time of [0.05, 0.6, 0.95]) {
    await page.evaluate(at => {
      window.BuildHero.hold();
      window.BuildHero.timeline.pause();
      window.BuildHero.timeline.time(at, false);
      for (const animation of document.getAnimations()) {
        if (animation.effect?.target?.matches(".hero-lane__body")) {
          animation.pause();
          animation.currentTime = at * 1000;
        }
      }
    }, time);
    const state = await page.evaluate(measureCoverage);
    assert.ok(Math.max(...state.gaps) <= 42, `${width} at ${time}s: empty bands ${state.gaps.map(gap => gap.toFixed(1))}px`);
    assert.deepEqual(state.shortTracks, [], `${width} at ${time}s: tracks fill the viewport while moving`);
    assert.notEqual(state.mask, "none", `${width}: notifications dissolve at the field boundary`);
    assert.ok(state.overflow <= 1, `${width}: no horizontal scroll`);
  }
  await page.evaluate(() => window.BuildHero.finish());
  assert.deepEqual(await page.locator("#act-1").boundingBox(), boxBefore, `${width}: entrance does not shift the hero`);
  assert.equal(await page.locator("[data-hero-field]").count(), 0, `${width}: opening clears before the copy is read`);
  assert.ok(await page.locator("#act-1-title").isVisible(), `${width}: headline remains visible`);
  const clickable = await page.locator("#act-1 .cta").evaluate(element => {
    const box = element.getBoundingClientRect();
    return element.contains(document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2));
  });
  assert.ok(clickable, `${width}: the hero CTA is under the pointer`);
  assert.deepEqual(errors, [], `${width}: no browser errors`);
  await context.close();
  console.log(`Notifications: ${width}×${height} passed`);
}

const browser = await chromium.launch({headless: true, executablePath: process.env.CHROMIUM_PATH});
try {
  for (const [width, height] of sizes) await checkField(browser, width, height);
} finally {
  await browser.close();
}
