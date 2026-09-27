// The GitHub repository picker on the New project sheet, driven in a real
// browser against the compose stack: the SPA greets the real bridge over the
// real transport, the bridge runs `gh` (a fake on its PATH that answers a
// fixed list), and the Git remote URL field must offer those repositories.
//
// Usage (host): node repo-picker-check.mjs   (APP defaults to localhost:8090)
import { chromium } from "playwright";

const APP = process.env.APP || "http://localhost:8090";
const EMAIL = process.env.QA_EMAIL || "qa@localhost";
const WANT = process.env.WANT_REPO || "zech/build";

const browser = await chromium.launch({ executablePath: "/usr/bin/chromium", headless: true, args: ["--no-sandbox"] });
const page = await (await browser.newContext({ viewport: { width: 1440, height: 900 } })).newPage();
page.on("pageerror", (e) => console.log("  [pageerror]", e.message));
page.on("console", (m) => { if (["error", "warning"].includes(m.type())) console.log("  [console]", m.type(), m.text().slice(0, 300)); });
let failed = false;
const note = (name, ok, detail = "") => { if (!ok) failed = true; console.log(`${ok ? "PASS" : "FAIL"} ${name}${detail ? "  — " + detail : ""}`); };
const state = () => page.evaluate(() => {
  const input = document.querySelector("[data-source-value]");
  return {
    input: Boolean(input),
    picker: input?.dataset.repoPicker || "",
    role: input?.getAttribute("role") || "",
    options: [...document.querySelectorAll('.repo-picker-list [role="option"] .repo-picker-name')].map((n) => n.textContent),
    note: [...document.querySelectorAll(".repo-picker-note")].map((n) => (n.hidden ? "" : n.textContent)).join(""),
    device: document.querySelector("#nrdevice")?.value ?? "(pinned)",
  };
});

try {
  await page.goto(`${APP}/auth/dummy/login`, { waitUntil: "load" });
  await page.fill('input[name="email"]', EMAIL);
  await page.fill('input[name="name"]', "Picker Check");
  await Promise.all([
    page.waitForNavigation({ waitUntil: "load", timeout: 15000 }).catch(() => {}),
    page.evaluate(() => document.querySelector('form[action="/auth/dummy-login"]').submit()),
  ]);
  await page.goto(`${APP}/app/`, { waitUntil: "load" });
  await page.waitForSelector("#inbox-new-project", { timeout: 60000 });
  // Give the session time to connect and greet.
  await page.waitForTimeout(Number(process.env.SETTLE_MS || 8000));
  // Clicked until the sheet opens: the button is painted from cache before
  // its handler is bound.
  const t0 = Date.now();
  while (!(await page.$("#nraddremote")) && Date.now() - t0 < 20000) {
    await page.evaluate(() => document.querySelector("#inbox-new-project").click());
    await page.waitForTimeout(100);
  }
  await page.waitForSelector("#nraddremote", { timeout: 1000 });
  console.log(`  sheet open ${Date.now() - t0} ms after the app loaded`);
  const select = await page.$("#nrdevice");
  if (select && !(await select.inputValue())) {
    const value = await page.evaluate(() => [...document.querySelectorAll("#nrdevice option")].map((o) => o.value).find(Boolean));
    await page.selectOption("#nrdevice", value);
  }
  await page.evaluate(() => document.querySelector("#nraddremote").click());
  await page.waitForTimeout(Number(process.env.ANSWER_MS || 6000));
  const opened = await state();
  console.log("  after Add Git remote:", JSON.stringify(opened));
  await page.focus("[data-source-value]");
  await page.keyboard.type(WANT.split("/")[1].slice(0, 4));
  await page.waitForTimeout(500);
  const typed = await state();
  console.log("  after typing:", JSON.stringify(typed));
  note("the Git remote URL field is a combobox", typed.role === "combobox", `role=${typed.role || "none"} picker=${typed.picker || "none"}`);
  note(`typing finds ${WANT}`, typed.options.includes(WANT), `options=${JSON.stringify(typed.options)} note=${JSON.stringify(typed.note)}`);
  await page.screenshot({ path: process.env.SHOT || "/home/zech/.build/project-scratch/rp-e2e/picker.png" });
} catch (error) {
  note("the check ran", false, error.message);
  await page.screenshot({ path: "/home/zech/.build/project-scratch/rp-e2e/fail.png" }).catch(() => {});
  console.log("  sheet:", await page.evaluate(() => document.querySelector("#sheet")?.innerHTML.slice(0, 400)).catch(() => ""));
} finally {
  await browser.close();
}
process.exit(failed ? 1 : 0);
