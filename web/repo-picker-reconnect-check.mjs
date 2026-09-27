// The New project sheet opened while this machine is reconnecting — its
// bridge restarting, the tab painting from cache as if connected — must still
// search the machine's GitHub repositories once the machine answers, within
// the same opening.
//
// Usage (host): node repo-picker-reconnect-check.mjs; when it prints
// RESTART-NOW, restart the bridge and touch $RESTARTED (default /tmp/rp-restarted).
import { existsSync } from "node:fs";
import { chromium } from "playwright";

const APP = process.env.APP || "http://localhost:8090";
const RESTARTED = process.env.RESTARTED || "/tmp/rp-restarted";
const browser = await chromium.launch({ executablePath: "/usr/bin/chromium", headless: true, args: ["--no-sandbox"] });
const page = await (await browser.newContext({ viewport: { width: 1440, height: 900 } })).newPage();
page.on("pageerror", (e) => console.log("  [pageerror]", e.message));
let failed = false;
const note = (name, ok, detail = "") => { if (!ok) failed = true; console.log(`${ok ? "PASS" : "FAIL"} ${name}${detail ? "  — " + detail : ""}`); };
const state = () => page.evaluate(() => ({
  role: document.querySelector("[data-source-value]")?.getAttribute("role") || "",
  options: [...document.querySelectorAll('.repo-picker-list [role="option"] .repo-picker-name')].map((n) => n.textContent),
  note: [...document.querySelectorAll(".repo-picker-note")].map((n) => (n.hidden ? "" : n.textContent)).join(""),
}));
try {
  await page.goto(`${APP}/auth/dummy/login`, { waitUntil: "load" });
  await page.fill('input[name="email"]', "qa@localhost");
  await page.fill('input[name="name"]', "Reconnect Check");
  await Promise.all([
    page.waitForNavigation({ waitUntil: "load", timeout: 15000 }).catch(() => {}),
    page.evaluate(() => document.querySelector('form[action="/auth/dummy-login"]').submit()),
  ]);
  await page.goto(`${APP}/app/`, { waitUntil: "load" });
  await page.waitForSelector("#inbox-new-project", { timeout: 60000 });
  await page.waitForTimeout(10000);
  console.log("RESTART-NOW");
  while (!existsSync(RESTARTED)) await page.waitForTimeout(200);
  await page.evaluate(() => document.querySelector("#inbox-new-project").click());
  await page.waitForSelector("#nraddremote", { timeout: 15000 });
  if (await page.$("#nrdevice") && !(await page.inputValue("#nrdevice"))) {
    const value = await page.evaluate(() => [...document.querySelectorAll("#nrdevice option")].map((o) => o.value).find(Boolean));
    await page.selectOption("#nrdevice", value);
  }
  await page.evaluate(() => document.querySelector("#nraddremote").click());
  const during = await state();
  console.log("  opened while reconnecting:", JSON.stringify(during));
  await page.waitForTimeout(Number(process.env.RECONNECT_MS || 30000));
  await page.focus("[data-source-value]");
  await page.keyboard.type("buil");
  await page.waitForTimeout(500);
  const after = await state();
  note("once the machine answers, the same opening searches its repositories", after.options.includes("zech/build"), JSON.stringify(after));
  await page.screenshot({ path: "/home/zech/.build/project-scratch/rp-e2e/reconnect.png" });
} catch (error) {
  note("the check ran", false, error.message);
} finally {
  await browser.close();
}
process.exit(failed ? 1 : 0);
