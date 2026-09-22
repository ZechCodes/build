// Record the reference scroll sequence and local frame/idle-render measurements.
import { fileURLToPath } from "node:url";
import fs from "node:fs/promises";
import { chromium } from "playwright";

const output = process.env.LANDING_REVIEW_DIR
  || fileURLToPath(new URL("../design/landing-review", import.meta.url));
const profiles = [["desktop", 1440, 900], ["mobile", 390, 844]];
const realGpuExpected = process.env.LANDING_HEADFUL === "1";

function monitorDrawCalls() {
  window.__drawCount = 0;
  for (const name of ["drawElements", "drawArrays"]) {
    const original = WebGL2RenderingContext.prototype[name];
    WebGL2RenderingContext.prototype[name] = function (...args) {
      window.__drawCount += 1;
      return original.apply(this, args);
    };
  }
}

async function waitForStory(page) {
  await page.waitForFunction(() => window.BuildLandingStory?.getState);
  const enhanced = await page.evaluate(() => window.BuildLandingStory.getState().enhanced);
  if (enhanced && realGpuExpected) {
    await page.waitForFunction(() => document.querySelector('[data-webgl-ready="true"]'));
  }
  return enhanced;
}

// Playwright runs this function in the page, so its imports and helpers live here.
async function recordStoryFrames() {
  const { travelAtFrame } = await import("/landing/story-manifest.js");
  const frames = [];
  const stage = document.querySelector("[data-story-stage]");
  const story = document.querySelector("[data-story]");
  const origin = story.getBoundingClientRect().top + scrollY;
  let last = performance.now();

  async function recordScene(index) {
    const profile = window.BuildLandingStory.getState().profile;
    const height = stage.getBoundingClientRect().height;
    const start = travelAtFrame(index, 0, profile) * height;
    const end = travelAtFrame(index, 1, profile) * height;
    const duration = index === 4 ? 6500 : 3500;
    await new Promise((resolve) => {
      const startedAt = performance.now();
      function tick(now) {
        frames.push(now - last);
        last = now;
        const progress = Math.min(1, (now - startedAt) / duration);
        scrollTo({ top: origin + start + (end - start) * progress, behavior: "instant" });
        if (progress < 1) requestAnimationFrame(tick);
        else resolve();
      }
      requestAnimationFrame(tick);
    });
  }

  for (let index = 0; index < 6; index += 1) await recordScene(index);
  const sorted = frames.slice(5).sort((a, b) => a - b);
  return {
    frameSamples: sorted.length,
    medianMs: sorted[Math.floor(sorted.length * 0.5)],
    p95Ms: sorted[Math.floor(sorted.length * 0.95)],
    over34ms: sorted.filter((value) => value > 34).length,
    state: window.BuildLandingStory.getState(),
  };
}

async function recordViewport(browser, [name, width, height]) {
  const context = await browser.newContext({
    viewport: { width, height },
    recordVideo: { dir: "/tmp/build-landing-review/recordings", size: { width, height } },
  });
  await context.addInitScript(monitorDrawCalls);
  const page = await context.newPage();
  await page.goto(process.env.LANDING_URL || "http://127.0.0.1:4173/", { waitUntil: "networkidle" });
  await waitForStory(page);
  await page.waitForTimeout(500);
  const drawsBefore = await page.evaluate(() => window.__drawCount);
  await page.waitForTimeout(500);
  const drawsAfter = await page.evaluate(() => window.__drawCount);
  const measurements = await page.evaluate(recordStoryFrames);
  await page.waitForTimeout(700);
  const video = page.video();
  await context.close();
  await video.saveAs(`${output}/${name}-animatic.webm`);
  const report = { viewport: [width, height], idleDrawCalls: [drawsBefore, drawsAfter], ...measurements };
  await fs.writeFile(`${output}/${name}-performance.json`, `${JSON.stringify(report, null, 2)}\n`);
  console.log(name, report);
}

async function captureOpeningStills(browser) {
  if (!realGpuExpected) return;
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await context.newPage();
  await page.goto(process.env.LANDING_URL || "http://127.0.0.1:4173/", { waitUntil: "networkidle" });
  const enhanced = await waitForStory(page);
  if (!enhanced) throw new Error("opening stills require the enhanced desktop story");
  await page.waitForFunction(() => (
    document.querySelector('[data-device="laptop"]')?.dataset.webglDeviceReady === "true"
  ));
  await page.waitForTimeout(400);

  for (const [file, local] of [["opening-closed.png", 0], ["opening-half.png", 0.225]]) {
    await page.evaluate(async (targetLocal) => {
      const { travelAtFrame } = await import("/landing/story-manifest.js");
      const story = document.querySelector("[data-story]");
      const stage = document.querySelector("[data-story-stage]");
      const state = window.BuildLandingStory.getState();
      const origin = story.getBoundingClientRect().top + scrollY;
      const top = origin + travelAtFrame(0, targetLocal, state.profile) * stage.getBoundingClientRect().height;
      scrollTo({ top, behavior: "instant" });
      await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    }, local);
    await page.screenshot({ path: `${output}/${file}`, animations: "disabled" });
  }
  await context.close();
}

await fs.mkdir(output, { recursive: true });
const browser = await chromium.launch({
  headless: process.env.LANDING_HEADFUL !== "1",
  executablePath: process.env.CHROMIUM_PATH,
  args: process.env.LANDING_HEADFUL === "1" ? ["--ozone-platform=x11"] : [],
});
try {
  for (const profile of profiles) await recordViewport(browser, profile);
  await captureOpeningStills(browser);
} finally {
  await browser.close();
}
