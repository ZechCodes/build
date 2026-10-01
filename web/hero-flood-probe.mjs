// Measure and record the hero's opening beat (#310). Start
// scripts/preview-landing.py first, as for web/landing-check.mjs.
//
//   LANDING_URL       where the preview listens (default http://127.0.0.1:4173)
//   HERO_PROBE_DIR    where results land (default /tmp/build-hero-probe)
//   HERO_PROBE_LABEL  a prefix for every file, e.g. "before" or "after"
//   CHROMIUM_PATH     a system Chromium instead of Playwright's download
//   CPU_THROTTLE      Chromium's CPU slowdown for the timing run (default 4)
//
// For each size it writes the field's DOM and compositor layer counts and
// the frame intervals of one throttled entrance without the film, phase by phase, to
// <label>-probe.json; a recording of an unthrottled entrance; and a strip of
// frames held on the entrance's own clock. It asserts nothing: the numbers
// are for a person to compare.
import fs from "node:fs/promises";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { chromium } from "playwright";

const base = process.env.LANDING_URL || "http://127.0.0.1:4173";
const output = process.env.HERO_PROBE_DIR || "/tmp/build-hero-probe";
const label = process.env.HERO_PROBE_LABEL || "probe";
const throttle = Number(process.env.CPU_THROTTLE || 4);
await fs.mkdir(output, { recursive: true });

const SIZES = [
  { name: "desktop", viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1 },
  { name: "phone", viewport: { width: 390, height: 844 }, deviceScaleFactor: 3, isMobile: true, hasTouch: true },
];
const HELD = [0.2, 0.6, 1.0, 1.4, 1.8, 2.6];

const browser = await chromium.launch({
  headless: true,
  executablePath: process.env.CHROMIUM_PATH,
  args: ["--headless=new", "--use-gl=angle", "--use-angle=gl", "--ignore-gpu-blocklist"],
});

// Every animation frame from the first, with the entrance's clock beside it.
function recordFrames() {
  const frames = (window.__heroFrames = []);
  const tick = (now) => {
    const hero = window.BuildHero;
    frames.push([now, hero && !hero.done ? hero.timeline.time() : null]);
    if (!hero?.done) requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

const contextOptions = ({ viewport, deviceScaleFactor, isMobile = false, hasTouch = false }) => ({ viewport, deviceScaleFactor, isMobile, hasTouch });

async function open(size, extra = {}) {
  const context = await browser.newContext({ ...contextOptions(size), ...extra });
  const page = await context.newPage();
  return { context, page };
}

const entranceUrl = (size) => `${base}/?hero=play${size.name === "desktop" ? "&film=1" : ""}`;
// The timing run leaves the film out: its stage loading on a throttled CPU
// would swamp the field's own cost.
const timingUrl = () => `${base}/?hero=play&film=0`;
const waitDone = (page) => page.waitForFunction(() => window.BuildHero === null || window.BuildHero?.done, null, { timeout: 120_000 });

function percentile(sorted, share) {
  return sorted.length ? sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * share))] : null;
}

function summarise(intervals) {
  const sorted = [...intervals].sort((a, b) => a - b);
  const round = (value) => (value === null ? null : Math.round(value * 10) / 10);
  return {
    frames: intervals.length,
    p50: round(percentile(sorted, 0.5)),
    p95: round(percentile(sorted, 0.95)),
    max: round(sorted.at(-1) ?? null),
    over25ms: intervals.filter((value) => value > 25).length,
  };
}

// Frame intervals while the entrance plays, split at the ripple and at the
// moment the ripple has passed.
async function frameTiming(size) {
  const { context, page } = await open(size);
  const session = await context.newCDPSession(page);
  await session.send("Emulation.setCPUThrottlingRate", { rate: throttle });
  await page.addInitScript(recordFrames);
  await page.goto(timingUrl(), { waitUntil: "commit" });
  await waitDone(page);
  const { frames, timing } = await page.evaluate(() => ({ frames: window.__heroFrames, timing: window.BuildHero?.timing }));
  await context.close();
  if (!timing) return { played: false };
  const phases = { field: [], ripple: [], after: [] };
  for (let index = 1; index < frames.length; index += 1) {
    const [now, clock] = frames[index];
    if (clock === null) continue;
    const phase = clock < timing.ripple[0] ? "field" : clock < timing.ripple[1] ? "ripple" : "after";
    phases[phase].push(now - frames[index - 1][0]);
  }
  return Object.fromEntries(Object.entries(phases).map(([phase, intervals]) => [phase, summarise(intervals)]));
}

async function holdAt(page, at) {
  await page.evaluate((time) => {
    const hero = window.BuildHero;
    hero.hold();
    hero.timeline.pause();
    hero.timeline.time(time, false);
  }, at);
  await page.waitForTimeout(200);
}

// The field's size and the compositor's layers mid-field.
async function counts(size) {
  const { context, page } = await open(size);
  await page.goto(entranceUrl(size), { waitUntil: "load" });
  await page.waitForFunction(() => window.BuildHero !== undefined, null, { timeout: 60_000 });
  await holdAt(page, 0.6);
  const session = await context.newCDPSession(page);
  const layers = new Promise((resolve) => session.on("LayerTree.layerTreeDidChange", ({ layers: tree }) => tree && resolve(tree)));
  await session.send("LayerTree.enable");
  await page.evaluate(() => { document.body.style.outlineColor = "transparent"; });
  const tree = await Promise.race([layers, new Promise((resolve) => setTimeout(() => resolve([]), 3000))]);
  const field = await page.evaluate(() => {
    const element = document.querySelector("[data-hero-field]");
    const below = document.querySelector(".site-nav").getBoundingClientRect().bottom;
    const pills = [...element.querySelectorAll(".hero-pill")];
    const visible = pills.filter((pill) => {
      const box = pill.getBoundingClientRect();
      return box.width > 0 && box.right > 0 && box.left < innerWidth && box.bottom > below && box.top < innerHeight;
    });
    return { nodes: element.querySelectorAll("*").length, pills: pills.length, visiblePills: visible.length, page: document.querySelectorAll("*").length };
  });
  await context.close();
  return { ...field, layers: tree.length, drawingLayers: tree.filter((layer) => layer.drawsContent).length };
}

async function record(size) {
  const dir = path.join(output, `${label}-${size.name}-video`);
  const { context, page } = await open(size, { recordVideo: { dir, size: size.viewport } });
  await page.goto(entranceUrl(size), { waitUntil: "commit" });
  await waitDone(page);
  await page.waitForTimeout(600);
  const video = page.video();
  await context.close();
  const file = path.join(output, `${label}-${size.name}.webm`);
  await fs.rename(await video.path(), file);
  await fs.rm(dir, { recursive: true, force: true });
  return file;
}

async function strip(size) {
  const { context, page } = await open(size);
  await page.goto(entranceUrl(size), { waitUntil: "load" });
  await page.waitForFunction(() => window.BuildHero !== undefined, null, { timeout: 60_000 });
  const shots = [];
  for (const at of HELD) {
    await holdAt(page, at);
    const shot = path.join(output, `${label}-${size.name}-${at.toFixed(1)}s.png`);
    await page.screenshot({ path: shot });
    shots.push(shot);
  }
  await context.close();
  const file = path.join(output, `${label}-${size.name}-frames.png`);
  const tile = size.name === "desktop" ? "3x2" : "6x1";
  const geometry = size.name === "desktop" ? "720x450+6+6" : "390x844+6+6";
  execFileSync("magick", ["montage", ...shots, "-tile", tile, "-geometry", geometry, "-background", "#222", file]);
  return file;
}

const results = {};
try {
  for (const size of SIZES) {
    results[size.name] = {
      counts: await counts(size),
      timing: await frameTiming(size),
      video: await record(size),
      frames: await strip(size),
    };
    console.log(size.name, JSON.stringify(results[size.name], null, 1));
  }
  await fs.writeFile(path.join(output, `${label}-probe.json`), JSON.stringify({ throttle, results }, null, 2));
} finally {
  await browser.close();
}
