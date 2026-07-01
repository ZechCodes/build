// End-to-end through the REAL web app: Skrift dummy login → the Build SPA →
// new task goal → planning agent → approve plan → coding agent → live git diff.
// Screenshots each stage to /tmp/build-app-*.png.
//
// Prereqs (relay-direct topology, gateway retired):
//   - skriftapp on APP (dummy auth enabled) serving the built SPA at /app/
//     (cd spa && npm run build)
//   - the Rust relay on the SPA's VITE_RELAY_URL (default ws://localhost:18090)
//   - a paired bridge online (real-agent mode or BRIDGE_QA_AGENT=1)
//
// Usage: APP=http://localhost:8090 node skrift-flow.mjs

import { chromium } from "playwright";

const APP = process.env.APP || "http://localhost:8090";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let fail = 0;
const check = (n, ok, d = "") => { console.log(`${ok ? "✓" : "✗"} ${n}${d ? "  — " + d : ""}`); if (!ok) fail++; };

const browser = await chromium.launch({ headless: true });
const page = await browser.newPage({ viewport: { width: 1500, height: 920 } });
page.on("console", (m) => { if (m.type() === "error") console.log("[page]", m.text()); });

try {
  // 1. Skrift auth: complete the dummy login form (the browser carries CSRF).
  await page.goto(`${APP}/auth/dummy/login`, { waitUntil: "load" });
  await page.fill('input[name="email"]', "demo@localhost");
  await page.fill('input[name="name"]', "Demo");
  await Promise.all([
    page.waitForNavigation({ waitUntil: "load", timeout: 15000 }).catch(() => {}),
    page.evaluate(() => document.querySelector('form[action="/auth/dummy-login"]').submit()),
  ]);
  await sleep(500);
  check("Skrift dummy login succeeded", !page.url().includes("/auth/login"), page.url());

  // 2. The authed Build SPA loads, passes the device gate, and connects over E2EE.
  await page.goto(`${APP}/app/`, { waitUntil: "load" });
  await page.waitForFunction(() => document.getElementById("conn")?.textContent === "connected", null, { timeout: 25000 });
  check("Skrift auth → Build SPA loads + connects over E2EE", true);
  await page.screenshot({ path: "/tmp/build-app-1-connected.png" });

  // 3. New task: goal in, dispatch, planning agent drafts the plan.
  await page.click("#newtask");
  await page.fill("#goal", 'Add a hello() function in a new file greeting.py that returns "Hello!", with a pytest test.');
  await page.click("#dispatch");
  console.log("  planning agent working…");
  await page.waitForSelector("#approvePlan", { timeout: 240000 });
  const planLen = await page.evaluate(() => document.getElementById("planbody")?.textContent.length || 0);
  check("planning agent produced a plan", planLen > 80, `${planLen} chars`);
  await page.screenshot({ path: "/tmp/build-app-2-plan.png" });

  // 4. Approve → coding agent implements → the diff tab fills with real changes.
  await page.click("#approvePlan");
  console.log("  coding agent implementing (watching diffs)…");
  await page.waitForSelector("#gitprimary", { timeout: 360000 });
  await page.waitForSelector(".file", { timeout: 15000 }).catch(() => {});
  const files = await page.$$eval(".file .fhead", (els) => els.map((e) => e.textContent.replace(/\s+/g, " ").trim()));
  check("coding agent produced a git diff", files.length > 0, files.join(", "));
  const diffLen = await page.evaluate(() => document.getElementById("tabbody")?.textContent.length || 0);
  check("diff content rendered", diffLen > 50, `${diffLen} chars`);
  await page.screenshot({ path: "/tmp/build-app-3-diff.png" });
} catch (e) {
  console.error("FLOW ERROR:", e.message);
  await page.screenshot({ path: "/tmp/build-app-error.png" }).catch(() => {});
  fail++;
} finally {
  await browser.close();
}

console.log("\nscreenshots: /tmp/build-app-{1-connected,2-plan,3-diff}.png");
if (fail) { console.error(`WEB APP FLOW FAIL: ${fail} check(s) failed`); process.exit(1); }
console.log("WEB APP FLOW PASS: Skrift login → goal → plan → approve → coding agent → live diffs");
process.exit(0);
