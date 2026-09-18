import { createRequire } from "node:module";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";

const require = createRequire(new URL("../../web/package.json", import.meta.url));
const { chromium } = require("playwright");
const origin = process.env.CAPTURE_ORIGIN || "http://127.0.0.1:4178";

const output = resolve("design/landing-captures/masters");
await mkdir(output, { recursive: true });

const launchOptions = { headless: true };
if (process.env.CHROMIUM_PATH) launchOptions.executablePath = process.env.CHROMIUM_PATH;
const browser = await chromium.launch(launchOptions);
const scenes = ["ui01", "ui02", "ui03", "ui04", "ui05"];
const profiles = [
  { name: "desktop", width: 1440, height: 900 },
  { name: "mobile", width: 390, height: 844 },
];

for (const profile of profiles) {
  const page = await browser.newPage({ viewport: { width: profile.width, height: profile.height }, deviceScaleFactor: 2 });
  for (const scene of scenes) {
    const states = scene === "ui03" ? ["question", "answer", "resumed"] : scene === "ui05" ? ["approval", "merged"] : ["default"];
    for (const state of states) {
      const suffix = state === "default" ? "" : `-${state}`;
      await page.goto(`${origin}/design/landing-captures/?scene=${scene}&state=${state}`);
      await page.evaluate(() => document.fonts.ready);
      await page.screenshot({ path: resolve(output, `${scene}${suffix}-${profile.name}.png`) });
    }
  }
  await page.close();
}

await browser.close();
