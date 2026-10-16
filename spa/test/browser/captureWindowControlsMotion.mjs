// Review evidence only; circles and the 84px reservation are synthetic.
// Run: node test/browser/captureWindowControlsMotion.mjs /tmp/635-motion-before before
import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { openLayoutSession } from "./layoutHarness.mjs";
import { navigationAppShim } from "./navigationBreadcrumbSeed.mjs";
import { mountWindowControls } from "./windowControlsSeed.mjs";

const output = resolve(process.argv[2] || "/tmp/635-motion");
const label = process.argv[3] || "motion";
const evidence = {
  label, capturedAt: new Date().toISOString(), viewport: { width: 1280, height: 800 },
  motion: "no-preference", activation: "mounted #inbox-collapse HTMLElement.click() handler",
  simulation: { reservation: 84, lightLeft: 12, lightTop: 16, lightSize: 12, lightGap: 8 },
  cssSha256: createHash("sha256").update(await readFile(new URL("../../src/styles/window-controls.css", import.meta.url))).digest("hex"),
  captures: [], scenarios: [],
};

async function beginScenario(page, name, clicks = []) {
  await page.evaluate(({ name, clicks }) => {
    window.__motionCaptureObserver?.disconnect();
    const state = { name, started: performance.now(), clicks: [], classChanges: [] };
    window.__motionCapture = state;
    window.__motionCaptureObserver = new MutationObserver(() => {
      state.classChanges.push({ elapsed: performance.now() - state.started, classes: document.body.className });
    });
    window.__motionCaptureObserver.observe(document.body, { attributes: true, attributeFilter: ["class"] });
    for (const offset of clicks) setTimeout(() => {
      state.clicks.push({ planned: offset, elapsed: performance.now() - state.started,
        collapsedBefore: document.body.classList.contains("inbox-collapsed") });
      // Use the production handler and its cache readback, never its controller directly.
      document.querySelector("#inbox-collapse").click();
    }, offset);
  }, { name, clicks });
}

function measure(page) {
  return page.evaluate(() => {
    const rect = (selector) => {
      const { x, y, width, height, right, bottom } = document.querySelector(selector).getBoundingClientRect();
      return { x, y, width, height, right, bottom };
    };
    const head = getComputedStyle(document.querySelector(".inbox-head"));
    const button = getComputedStyle(document.querySelector("#inbox-new-project"));
    const span = document.querySelector("#inbox-new-project span");
    const range = document.createRange();
    range.selectNodeContents(span);
    return {
      elapsed: performance.now() - window.__motionCapture.started,
      classes: document.body.className, transitioning: document.body.classList.contains("inbox-transitioning"),
      collapsed: document.body.classList.contains("inbox-collapsed"),
      rail: rect("#inbox-rail"), head: rect(".inbox-head"), newProject: rect("#inbox-new-project"),
      headerPadding: [head.paddingTop, head.paddingRight, head.paddingBottom, head.paddingLeft],
      newProjectGap: button.gap, newProjectPadding: [button.paddingLeft, button.paddingRight],
      labelLines: range.getClientRects().length, labelWhiteSpace: getComputedStyle(span).whiteSpace,
      toolbarPadding: getComputedStyle(document.querySelector(".toolbar")).paddingLeft,
    };
  });
}

async function captureAt(page, name, elapsed = 0) {
  await page.evaluate((elapsed) => new Promise((done) => {
    setTimeout(done, Math.max(0, elapsed - (performance.now() - window.__motionCapture.started)));
  }), elapsed);
  const path = join(output, `${label}-${name}.png`);
  const before = await measure(page);
  await page.screenshot({ path, animations: "allow" });
  evidence.captures.push({ name, path, plannedElapsed: elapsed, before, after: await measure(page) });
}

await mkdir(output, { recursive: true });
const session = await openLayoutSession({ plugins: [navigationAppShim] });
try {
  await session.withPage(async ({ page, basePath }) => {
    await page.emulateMedia({ reducedMotion: "no-preference" });
    await mountWindowControls(page, basePath, { pinned: true, lights: true });
    await page.mouse.move(1279, 799);
    await page.waitForTimeout(300);
    await beginScenario(page, "unpin");
    await captureAt(page, "unpin-before-click");
    await beginScenario(page, "unpin", [0]);
    await captureAt(page, "unpin-hold", 120);
    await captureAt(page, "unpin-moving", 280);
    await captureAt(page, "unpin-settled", 600);
    evidence.scenarios.push(await page.evaluate(() => window.__motionCapture));
    await beginScenario(page, "rapid-pin-unpin-pin");
    await captureAt(page, "rapid-before-click");
    await beginScenario(page, "rapid-pin-unpin-pin", [0, 60, 120]);
    await captureAt(page, "rapid-middle-unpin", 90);
    await captureAt(page, "rapid-reversal-hold", 160);
    await captureAt(page, "rapid-moving", 320);
    await captureAt(page, "rapid-settled", 700);
    evidence.scenarios.push(await page.evaluate(() => window.__motionCapture));
  }, evidence.viewport);
} finally {
  await session.close();
  await writeFile(join(output, `${label}-measurements.json`), `${JSON.stringify(evidence, null, 2)}\n`);
  const rows = evidence.captures.map(({ name, before, after }) =>
    `${name}: ${before.elapsed.toFixed(1)}–${after.elapsed.toFixed(1)}ms, head=${before.head.height}px, top=${before.head.y.toFixed(1)}px, padding=${before.headerPadding.join(" ")}, action gap=${before.newProjectGap}, label lines=${before.labelLines}, transitioning=${before.transitioning}`);
  await writeFile(join(output, `${label}-measurements.txt`), [
    `Synthetic native controls; production shell and mounted pin handler; ${evidence.motion} motion.`,
    `CSS SHA-256: ${evidence.cssSha256}`, ...rows, "",
  ].join("\n"));
}
console.log(output);
