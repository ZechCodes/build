import { createRequire } from "node:module";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { spawn } from "node:child_process";

const require = createRequire(new URL("../../web/package.json", import.meta.url));
const { chromium } = require("playwright");
const origin = process.env.CAPTURE_ORIGIN || "http://127.0.0.1:4178";
const manifest = JSON.parse(await readFile(new URL("./screen-manifest.json", import.meta.url), "utf8"));

const output = resolve("design/landing-captures/masters");
const derivatives = resolve("skriftapp/buildapp/landing/assets/screens");
const reportPath = resolve("design/landing-captures/capture-results.json");
await mkdir(output, { recursive: true });
await mkdir(derivatives, { recursive: true });

const launchOptions = { headless: true };
if (process.env.CHROMIUM_PATH) launchOptions.executablePath = process.env.CHROMIUM_PATH;
const browser = await chromium.launch(launchOptions);
const allScenes = ["ui03", "ui05", "ui09-needs-you", "ui10-editor", "ui12-tasks", "ui13-team", "ui14-git", "ui15-triage", "ui16-builder"];
const allProfiles = [
  { name: "macbook", width: 1512, height: 982, scale: 2, native: [3024, 1964] },
  { name: "ipad", width: 1210, height: 834, scale: 2, native: [2420, 1668] },
  { name: "iphone", width: 440, height: 956, scale: 3, native: [1320, 2868] },
];
const selectedNames = (variable, available) => {
  if (!variable) return available;
  const requested = new Set(variable.split(",").map((name) => name.trim()).filter(Boolean));
  const selected = available.filter((entry) => requested.has(typeof entry === "string" ? entry : entry.name));
  if (!selected.length) throw new Error(`No capture targets matched ${variable}`);
  return selected;
};
const scenes = selectedNames(process.env.CAPTURE_SCENES, allScenes);
const profiles = selectedNames(process.env.CAPTURE_PROFILES, allProfiles);
const requestedStates = process.env.CAPTURE_STATES
  ? new Set(process.env.CAPTURE_STATES.split(",").map((name) => name.trim()).filter(Boolean))
  : null;
const captureResults = [];

// Quality 80: the app's layout is denser than the storyboard's was, and at 80
// the thirteen screens the film draws weigh what they did before, with the
// text as sharp as at 88.
const resizeWebp = (source, destination, width, height) => new Promise((resolvePromise, reject) => {
  const imageMagick = spawn("magick", [source, "-resize", `${width}x${height}!`, "-quality", "80", destination], { stdio: "inherit" });
  imageMagick.once("error", reject);
  imageMagick.once("exit", (code) => code === 0 ? resolvePromise() : reject(new Error(`ImageMagick exited with ${code}`)));
});

async function assertSystemSafeAreas(page, profile) {
  // The profile name is passed as an argument, never spliced into the page script.
  // nosemgrep: javascript.playwright.security.audit.playwright-evaluate-arg-injection.playwright-evaluate-arg-injection
  const result = await page.evaluate((profileName) => {
    const rectangle = (selector) => {
      const element = document.querySelector(selector);
      if (!element) return null;
      const { left, right, top, bottom, width, height } = element.getBoundingClientRect();
      return { left, right, top, bottom, width, height };
    };
    const overlaps = (first, second) => first && second
      && first.left < second.right && first.right > second.left
      && first.top < second.bottom && first.bottom > second.top;
    const appHostIssues = (host, app) => {
      if (!host || !app) return ["missing app host or app"];
      if (Math.abs(host.width - app.width) > .5 || Math.abs(host.height - app.height) > .5) {
        return ["app does not fill its reserved host"];
      }
      return [];
    };
    const hostSelector = profileName === "iphone" ? ".iphone-app-host" : ".system-app-host";
    const host = rectangle(hostSelector);
    const app = rectangle(`${hostSelector} > .app`);
    const exclusions = profileName === "macbook"
      ? { menuBar: rectangle(".mac-menu-bar"), cameraHousing: rectangle(".camera-safe-area"), dock: rectangle(".mac-dock") }
      : profileName === "ipad"
        ? { statusBar: rectangle(".ipad-status-bar"), windowControls: rectangle(".ipad-window-controls"), dock: rectangle(".ipad-dock"), homeIndicator: rectangle(".home-indicator") }
        : { statusBar: rectangle(".iphone-status-bar"), islandSafeArea: rectangle(".island-safe-area"), homeSafeArea: rectangle(".iphone-home-safe"), homeIndicator: rectangle(".iphone-home-safe .home-indicator") };
    const issues = appHostIssues(host, app);
    Object.entries(exclusions).forEach(([name, exclusion]) => {
      if (!exclusion) issues.push(`missing system exclusion ${name}`);
      else if (overlaps(host, exclusion)) issues.push(`app host overlaps system exclusion ${name}`);
    });
    const statusChildren = profileName === "iphone"
      ? [rectangle(".iphone-status-bar > strong"), rectangle(".island-safe-area"), rectangle(".iphone-status-bar .system-status-right")]
      : [];
    if (statusChildren.length && (statusChildren.some((entry) => !entry)
      || overlaps(statusChildren[0], statusChildren[1])
      || overlaps(statusChildren[1], statusChildren[2]))) {
      issues.push("iPhone status content enters the island exclusion");
    }
    return { issues, host, exclusions, viewport: { width: innerWidth, height: innerHeight } };
  }, profile.name);
  if (result.viewport.width !== profile.width || result.viewport.height !== profile.height) {
    result.issues.push(`viewport ${result.viewport.width}x${result.viewport.height} is not ${profile.width}x${profile.height}`);
  }
  const expectedBounds = manifest.profiles.system[profile.name].appBounds;
  const actualBounds = result.host && [result.host.left, result.host.top, result.host.width, result.host.height];
  if (!actualBounds || actualBounds.some((value, index) => Math.abs(value - expectedBounds[index]) > .5)) {
    result.issues.push(`app bounds ${JSON.stringify(actualBounds)} do not match manifest ${JSON.stringify(expectedBounds)}`);
  }
  if (profile.width * profile.scale !== profile.native[0] || profile.height * profile.scale !== profile.native[1]) {
    result.issues.push(`capture scale does not produce native dimensions ${profile.native.join("x")}`);
  }
  return result;
}

const reportRectangle = (rectangle) => rectangle
  ? [rectangle.left, rectangle.top, rectangle.width, rectangle.height].map((value) => Math.round(value * 100) / 100)
  : null;

for (const profile of profiles) {
  const page = await browser.newPage({ viewport: { width: profile.width, height: profile.height }, deviceScaleFactor: profile.scale });
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  for (const scene of scenes) {
    const availableStates = scene === "ui03" ? ["question", "answer", "resumed"] : scene === "ui05" ? ["approval", "merged"] : ["default"];
    const states = requestedStates ? availableStates.filter((name) => requestedStates.has(name)) : availableStates;
    for (const state of states) {
      const suffix = state === "default" ? "" : `-${state}`;
      // Loopback capture page; scene, state and profile come from the fixed lists above.
      // nosemgrep: javascript.playwright.security.audit.playwright-goto-injection.playwright-goto-injection
      await page.goto(`${origin}/design/landing-captures/?scene=${scene}&state=${state}&profile=${profile.name}`);
      await page.evaluate(() => document.fonts.ready);
      const viteOverlay = await page.locator("vite-error-overlay").count();
      if (pageErrors.length || viteOverlay) {
        throw new Error(`Fixture failed to render ${scene}${suffix}-${profile.name}: ${pageErrors.join("; ") || "Vite error overlay"}`);
      }
      const validation = await assertSystemSafeAreas(page, profile);
      const stem = `${scene}${suffix}-${profile.name}`;
      // Where each Tasks dashboard row sits on the texture: the landing hero
      // lands its notifications on these (landing/src/hero/anchors.js).
      const taskRows = await page.evaluate(() => [...document.querySelectorAll(".task-dashboard-row[data-task]")].map((row) => {
        const { left, top, width, height } = row.getBoundingClientRect();
        return { task: row.dataset.task, rect: [left, top, width, height].map((value) => Math.round(value * 100) / 100) };
      }));
      captureResults.push({
        state: `${scene}${suffix}`,
        profile: profile.name,
        logicalViewport: [profile.width, profile.height],
        deviceScaleFactor: profile.scale,
        nativeDimensions: profile.native,
        appBounds: reportRectangle(validation.host),
        systemExclusions: Object.fromEntries(Object.entries(validation.exclusions).map(([name, rectangle]) => [name, reportRectangle(rectangle)])),
        issues: validation.issues,
        ...(taskRows.length ? { taskRows } : {}),
      });
      const masterPath = resolve(output, `${stem}.png`);
      await page.screenshot({ path: masterPath });
      await resizeWebp(masterPath, resolve(derivatives, `${stem}.webp`), profile.width, profile.height);
    }
  }
  await page.close();
}

await browser.close();
let results = captureResults;
if (process.env.CAPTURE_SCENES || process.env.CAPTURE_PROFILES || process.env.CAPTURE_STATES) {
  try {
    const previous = JSON.parse(await readFile(reportPath, "utf8")).results || [];
    const replaced = new Set(captureResults.map(({ state, profile }) => `${state}:${profile}`));
    results = [...previous.filter(({ state, profile }) => !replaced.has(`${state}:${profile}`)), ...captureResults]
      .sort((a, b) => a.state.localeCompare(b.state) || a.profile.localeCompare(b.profile));
  } catch {
    // A first targeted capture simply starts the report with its selected assets.
  }
}
await writeFile(reportPath, `${JSON.stringify({
  version: 1,
  generatedBy: "design/landing-captures/capture.mjs",
  checkedAssets: results.length,
  results,
}, null, 2)}\n`);

const issues = captureResults.flatMap((result) => result.issues.map((issue) => `${result.state}-${result.profile}: ${issue}`));
if (issues.length) throw new Error(`Capture validation failed:\n${issues.join("\n")}`);
