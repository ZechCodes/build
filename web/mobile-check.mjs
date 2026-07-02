// Mobile readiness check: drives the REAL web app in an emulated phone
// (390×844, touch, mobile UA) through the full product surface and asserts
// three things everywhere: functionality is VISIBLE, INTERACTABLE via touch,
// and pages SCROLL correctly (vertical only — no horizontal page overflow;
// wide diffs scroll inside their own container).
//
// Prereqs: the compose stack (app :8090, relay :18090, QA bridge) up, and the
// bridge paired to QA_EMAIL (default demo@localhost):
//   QA_EMAIL=demo@localhost PAIRING_CODE=COMPOSE-PAIR node pair.mjs
//
// Usage: APP=http://localhost:8090 node mobile-check.mjs

import { chromium, devices } from "playwright";

const APP = process.env.APP || "http://localhost:8090";
const EMAIL = process.env.QA_EMAIL || "demo@localhost";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let fail = 0;
const check = (name, ok, detail = "") => {
  console.log(`${ok ? "✓" : "✗"} ${name}${detail ? "  — " + detail : ""}`);
  if (!ok) fail++;
};

const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({
  ...devices["iPhone 13"], // 390×844, touch, mobile UA
  // Playwright's chromium can't switch to webkit UA rendering, but viewport,
  // touch, and DPR are what the CSS + interaction paths depend on.
});
const page = await context.newPage();
page.on("console", (m) => {
  if (m.type() === "error") console.log("[page]", m.text());
});

/** No horizontal page scroll — the core "scrolls correctly" invariant. */
async function assertNoHScroll(label) {
  const { sw, iw } = await page.evaluate(() => ({
    sw: document.documentElement.scrollWidth,
    iw: window.innerWidth,
  }));
  check(`${label}: no horizontal page overflow`, sw <= iw + 1, `scrollWidth=${sw} innerWidth=${iw}`);
}

/** Element is visible AND inside the viewport horizontally. */
async function assertVisible(selector, label) {
  const el = page.locator(selector).first();
  const visible = await el.isVisible().catch(() => false);
  check(`${label} visible`, visible);
  if (!visible) return false;
  const box = await el.boundingBox();
  const iw = page.viewportSize().width;
  check(`${label} within viewport width`, box.x >= -1 && box.x + Math.min(box.width, 1) <= iw + 1, `x=${Math.round(box.x)} w=${Math.round(box.width)}`);
  return true;
}

/** Every visible text-entry field must be >=16px or iOS zooms on focus. */
async function assertFieldFontSizes(label) {
  const small = await page.evaluate(() =>
    [...document.querySelectorAll("input:not([type=checkbox]), select, textarea")]
      .filter((el) => el.offsetParent !== null)
      .map((el) => ({ tag: el.tagName + (el.id ? "#" + el.id : "." + el.className), px: parseFloat(getComputedStyle(el).fontSize) }))
      .filter((f) => f.px < 16),
  );
  check(`${label}: all fields >=16px (no iOS focus zoom)`, small.length === 0, small.map((f) => `${f.tag}=${f.px}px`).join(" "));
}

try {
  // ---- 1. Login (dummy) + connect over E2EE ----
  await page.goto(`${APP}/auth/dummy/login`, { waitUntil: "load" });
  await page.fill('input[name="email"]', EMAIL);
  await page.fill('input[name="name"]', "Mobile QA");
  await Promise.all([
    page.waitForNavigation({ waitUntil: "load", timeout: 15000 }).catch(() => {}),
    page.evaluate(() => document.querySelector('form[action="/auth/dummy-login"]').submit()),
  ]);
  await page.goto(`${APP}/app/`, { waitUntil: "load" });
  await page.waitForFunction(() => document.getElementById("conn")?.textContent?.includes("connected"), null, { timeout: 30000 });
  check("login + E2EE session on a phone viewport", true);
  await assertNoHScroll("board");

  // ---- 2. PWA manifest + icons (iOS push/install prerequisite) ----
  for (const path of ["/app/static/manifest.webmanifest", "/app/static/icon-192.png", "/app/static/apple-touch-icon.png", "/app/sw.js"]) {
    const status = await page.evaluate(async (p) => (await fetch(p)).status, path);
    check(`asset ${path}`, status === 200, `status ${status}`);
  }
  const manifest = await page.evaluate(async () => await (await fetch("/app/static/manifest.webmanifest")).json());
  check("manifest is installable-shaped", manifest.display === "standalone" && manifest.icons?.length >= 2);

  // ---- 3. Nav: all tabs tappable, board renders ----
  await assertVisible("#nav-board", "nav Board");
  await assertVisible("#nav-settings", "nav Settings");
  await page.tap("#nav-settings");
  await sleep(300);
  await assertNoHScroll("settings");
  await assertVisible("#nav-notif", "nav Notifications");
  await page.tap("#nav-notif");
  await sleep(300);
  await assertNoHScroll("notifications");
  await page.tap("#nav-board");
  await sleep(300);

  // ---- 4. Dispatch sheet: goal + project + model + effort, all touch ----
  await page.tap("#newtask");
  await assertVisible("#goal", "goal textarea");
  await assertVisible("#project", "project select");
  await assertVisible("#model", "model select");
  await assertVisible("#effort", "effort select");
  // Unique per run: the QA agent's output is goal-derived, so a repeated goal
  // on a reused stack produces an empty diff after the first merge.
  await page.fill("#goal", `Add a mobile-check greeting file (run ${Date.now()})`);
  // The catalog populates async from the bridge (models.list) — wait for it.
  await page.waitForFunction(() => document.querySelectorAll("#model option").length > 1, null, { timeout: 10000 });
  const modelOptions = await page.$$eval("#model option", (os) => os.map((o) => o.value));
  check("model select offers the bridge catalog", modelOptions.includes("claude-opus-4-8"), modelOptions.join(","));
  await page.selectOption("#model", "claude-haiku-4-5");
  check("effort disables for no-effort model", await page.$eval("#effort", (e) => e.disabled));
  await page.selectOption("#model", "");
  check("effort re-enables on harness default", await page.$eval("#effort", (e) => !e.disabled));
  await assertNoHScroll("dispatch sheet");
  await assertFieldFontSizes("dispatch sheet");
  await page.tap("#dispatch");

  // ---- 5. Plan review: readable + touch commenting ----
  await page.waitForSelector("#approvePlan", { timeout: 120000 });
  await assertNoHScroll("plan tab");
  await assertVisible("#planbody", "plan body");
  // Touch selections arrive via selectionchange with no pointer event — set a
  // range programmatically to exercise exactly that path.
  await page.evaluate(() => {
    const el = document.querySelector("#planbody p, #planbody li, #planbody h1, #planbody h2, #planbody");
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    const node = walker.nextNode();
    const range = document.createRange();
    range.setStart(node, 0);
    range.setEnd(node, Math.min(12, node.textContent.length));
    const sel = window.getSelection();
    sel.removeAllRanges();
    sel.addRange(range);
  });
  await page.waitForSelector(".comment-pop", { timeout: 3000 });
  check("plan: selection popover appears from selectionchange (touch path)", true);
  await page.tap(".comment-pop .cp-add");
  await assertFieldFontSizes("plan tab + comment popover");
  await page.fill(".comment-pop .cp-input", "mobile plan comment");
  await page.tap(".comment-pop .cp-save");
  await page.waitForSelector("#pclist .pcomment", { timeout: 3000 });
  check("plan: comment added by touch", true);
  await assertVisible("#requestUpdates", "Request Updates button");
  await page.tap("#clearfb");
  await sleep(200);

  // ---- 6. Approve with model override controls ----
  await assertVisible("#apModel", "approve-bar model select");
  await assertVisible("#apEffort", "approve-bar effort select");
  await assertVisible("#approvePlan", "approve button");
  await page.tap("#approvePlan");

  // ---- 7. Diff review: contained horizontal scroll + tap-to-comment ----
  await page.waitForSelector(".file table tr[data-ln]", { timeout: 180000 });
  await sleep(500);
  await assertNoHScroll("diff tab");
  const containment = await page.evaluate(() => {
    const file = document.querySelector(".file");
    return { fileScrollable: file.scrollWidth >= file.clientWidth, pageW: document.documentElement.scrollWidth, innerW: window.innerWidth };
  });
  check("diff: wide code scrolls inside the file card, not the page", containment.pageW <= containment.innerW + 1);
  await page.tap(".file table tr[data-ln] td.code");
  await page.waitForSelector(".comment-pop", { timeout: 3000 });
  check("diff: tap on a line opens the comment popover", true);
  await page.tap(".comment-pop .cp-add");
  await assertFieldFontSizes("diff tab + comment popover");
  await page.fill(".comment-pop .cp-input", "mobile diff comment");
  await page.tap(".comment-pop .cp-save");
  await page.waitForSelector("#difflist .pcomment", { timeout: 3000 });
  check("diff: line comment added by touch", true);
  await page.tap("#difflist .pcx"); // remove it so approve stays available
  await sleep(200);

  // ---- 8. Terminal drawer: tap target + visibility (before merge — the
  // merge action navigates back to the board) ----
  await assertVisible("#termToggle", "terminal toggle");
  await page.tap("#termToggle");
  await page.waitForSelector("#drawer.show", { timeout: 10000 });
  check("terminal drawer opens by tap", true);
  const drawerBox = await page.locator("#drawer").boundingBox();
  check("terminal drawer fits viewport", drawerBox.width <= page.viewportSize().width + 1);
  await page.tap("#dx");
  await sleep(200);
  check("terminal drawer closes by tap", !(await page.$("#drawer.show")));

  // ---- 9. Merge via the split button; the app returns to the board ----
  await page.waitForSelector("#gitprimary", { timeout: 10000 });
  await assertVisible("#gitprimary", "merge button");
  await page.tap("#gitprimary");
  await page.waitForFunction(
    () => [...document.querySelectorAll(".card .chip")].some((c) => c.textContent === "MERGED"),
    null,
    { timeout: 60000 },
  );
  check("task merged from a phone (board shows MERGED)", true);

  // ---- 10. Board after the full flow still scrolls right ----
  await assertNoHScroll("board (after full flow)");
} catch (e) {
  console.error("MOBILE FLOW ERROR:", e.message);
  await page.screenshot({ path: "/tmp/mobile-check-error.png" }).catch(() => {});
  fail++;
}

await browser.close();
console.log(fail === 0 ? "MOBILE CHECK PASS" : `MOBILE CHECK FAIL (${fail})`);
process.exit(fail === 0 ? 0 : 1);
