// Capture the invite email's device screens from the landing capture fixture,
// writing only into design/email/ so the landing's masters stay as they are.
//
// Serve the repository root with Vite first (spa/node_modules/.bin/vite . --port 4178), then:
//   CHROMIUM_PATH=/usr/bin/chromium node design/email/capture_screens.mjs
//
// tablet-screen.png: the Changes view of archive-search at the iPad profile.
// phone-screen.png: the inbox on the iPhone, opened from its toggle the way the
// app opens it at that width, over the Implement conversation.
import { createRequire } from "node:module";
import { resolve } from "node:path";

const require = createRequire(new URL("../../web/package.json", import.meta.url));
const { chromium } = require("playwright");
const origin = process.env.CAPTURE_ORIGIN || "http://127.0.0.1:4178";

const SHOTS = [
  { file: "tablet-screen.png", scene: "ui14-git", state: "default", profile: "ipad", width: 1210, height: 834, scale: 2 },
  { file: "phone-screen.png", scene: "ui03", state: "resumed", profile: "iphone", width: 440, height: 956, scale: 3, inbox: true },
];

const launchOptions = { headless: true };
if (process.env.CHROMIUM_PATH) launchOptions.executablePath = process.env.CHROMIUM_PATH;
const browser = await chromium.launch(launchOptions);
for (const shot of SHOTS) {
  const page = await browser.newPage({ viewport: { width: shot.width, height: shot.height }, deviceScaleFactor: shot.scale });
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  // Loopback capture page; scene, state and profile come from the fixed list above.
  // nosemgrep: javascript.playwright.security.audit.playwright-goto-injection.playwright-goto-injection
  await page.goto(`${origin}/design/landing-captures/?scene=${shot.scene}&state=${shot.state}&profile=${shot.profile}`);
  await page.evaluate(() => document.fonts.ready);
  if (pageErrors.length || await page.locator("vite-error-overlay").count()) {
    throw new Error(`Fixture failed to render ${shot.file}: ${pageErrors.join("; ") || "Vite error overlay"}`);
  }
  if (shot.inbox) {
    await page.evaluate(() => {
      document.body.classList.add("inbox-popover-open");
      document.querySelector("#inbox-open")?.setAttribute("aria-expanded", "true");
    });
    // The popover eases in over 180 ms.
    await page.waitForTimeout(400);
  }
  const path = resolve("design/email", shot.file);
  await page.screenshot({ path });
  console.log(`${path}: ${shot.width * shot.scale}x${shot.height * shot.scale}`);
  await page.close();
}
await browser.close();
