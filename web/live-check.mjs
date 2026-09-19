// The three manual checks from the cache-first plan, driven in a real browser
// against the compose stack. Reads /tmp/live-seed.json (live-seed.mjs output).
//
//   1. reload paints the app from cache with no gate frame, bridge paused
//   2. a workspace switch shows bubbles and the thread on the first frames
//   3. a background write moves the active workspace's git surface within ~1 s
//      and a background workspace's inbox row within 30 s
//
// Usage (host): node live-check.mjs
import { readFileSync, writeFileSync } from "node:fs";
import { execSync } from "node:child_process";
import { chromium } from "/home/zech/.local/share/mise/installs/npm-playwright/1.63.0/node_modules/playwright/index.mjs";

const APP = process.env.APP || "http://localhost:8090";
const EMAIL = process.env.QA_EMAIL || "qa@localhost";
const seed = JSON.parse(readFileSync("/tmp/live-seed.json", "utf8"));
const [A, B] = seed.workspaces;
const docker = (cmd) => execSync(`echo '${cmd}' | newgrp docker`, { encoding: "utf8", stdio: ["pipe", "pipe", "pipe"] });
const results = [];
const note = (name, ok, detail = "") => { results.push({ name, ok, detail }); console.log(`${ok ? "✓" : "✗"} ${name}${detail ? "  — " + detail : ""}`); };
const route = (ws, tab = "") => `#/project/${seed.projectId}/workspace/${ws.workspaceId}${tab ? "/" + tab : ""}`;

const browser = await chromium.launch({ executablePath: "/usr/bin/chromium", headless: true, args: ["--no-sandbox"] });
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await context.newPage();
page.on("pageerror", (e) => console.log("  [pageerror]", e.message));
page.on("console", (m) => { if (m.type() === "error") console.log("  [console.error]", m.text().slice(0, 200)); });

// A sampler that runs INSIDE the page: from now, record at every animation
// frame whether each selector has a match, until all do or the budget ends.
const sampleUntil = (specs, budgetMs) => page.evaluate(({ specs, budgetMs }) => new Promise((resolve) => {
  const t0 = performance.now(); const seen = {}; let frames = 0;
  const gate = () => Boolean(document.body?.classList.contains("gated"));
  const h1 = () => document.querySelector("#root h1")?.textContent || "";
  const first = { gated: gate(), h1: h1() };
  let firstBodyFrame = null;
  const tick = () => {
    frames += 1; const now = performance.now() - t0;
    if (!document.body) return requestAnimationFrame(tick);
    if (firstBodyFrame === null) { firstBodyFrame = frames; first.gated = gate(); first.h1 = h1(); }
    for (const [key, spec] of Object.entries(specs)) {
      if (seen[key] !== undefined) continue;
      const scopeText = () => (spec.in ? document.querySelector(spec.in)?.textContent || "" : document.body.textContent || "");
      const ok = spec.text ? scopeText().includes(spec.text) : document.querySelectorAll(spec.sel).length >= (spec.min || 1);
      if (ok) seen[key] = { ms: Math.round(now), frame: frames };
    }
    if (Object.keys(seen).length === Object.keys(specs).length || now > budgetMs) return resolve({ seen, frames, first, firstBodyFrame, gatedNow: gate(), h1Now: h1() });
    requestAnimationFrame(tick);
  };
  tick();
}), { specs, budgetMs });


/** Poll the page's text from Node every 50 ms until it holds `text`, up to
 *  `budgetMs`. Answers the ms it took, or null. (An in-page rAF sampler was
 *  observed to miss text the page demonstrably showed; this is the plain way.) */
const pollText = async (text, budgetMs) => {
  const t0 = Date.now();
  while (Date.now() - t0 < budgetMs) {
    if ((await page.evaluate(() => document.body.textContent)).includes(text)) return Date.now() - t0;
    await page.waitForTimeout(50);
  }
  return null;
};
try {
  // ---- login + first live paint (this is also what warms the cache) ----
  await page.goto(`${APP}/auth/dummy/login`, { waitUntil: "load" });
  await page.fill('input[name="email"]', EMAIL);
  await page.fill('input[name="name"]', "Live Check");
  await Promise.all([
    page.waitForNavigation({ waitUntil: "load", timeout: 15000 }).catch(() => {}),
    page.evaluate(() => document.querySelector('form[action="/auth/dummy-login"]').submit()),
  ]);
  const t0 = Date.now();
  await page.goto(`${APP}/app/`, { waitUntil: "load" });
  await page.waitForFunction((names) => names.every((n) => document.body.innerText.includes(n)), [A.name, B.name], { timeout: 90000 });
  note("cold login paints both seeded workspaces", true, `${Date.now() - t0} ms after navigation`);
  // Open each workspace once so its thread and git surfaces are warmed.
  for (const ws of [A, B]) {
    await page.evaluate((h) => { location.hash = h; }, route(ws));
    await page.waitForFunction(() => document.querySelectorAll(".rail-strip .rail-bubble-label, .thread-items [data-sequence]").length > 0, null, { timeout: 60000 });
    await page.evaluate((h) => { location.hash = h; }, route(ws, "changes"));
    await page.waitForSelector(".gitpane", { timeout: 60000 });
    await page.waitForTimeout(1500);
  }
  await page.screenshot({ path: "/tmp/live-0-warm.png" });

  // ---- check 2: workspace switch, first frames ----
  const switches = [];
  for (const ws of [A, B, A, B]) {
    await page.evaluate((h) => { location.hash = h; }, route(ws));
    const r = await sampleUntil({ bubbles: { sel: ".rail-strip .rail-bubble-label" }, thread: { sel: ".thread-items [data-sequence]" } }, 5000);
    switches.push({ to: ws.name, ...r.seen });
  }
  const worst = Math.max(...switches.map((s) => Math.max(s.bubbles?.ms ?? 9999, s.thread?.ms ?? 9999)));
  note("workspace switch shows bubbles and thread within the first frames", worst <= 100, switches.map((s) => `${s.to.split("-")[1]}: bubbles ${s.bubbles ? s.bubbles.ms + "ms/f" + s.bubbles.frame : "never"}, thread ${s.thread ? s.thread.ms + "ms/f" + s.thread.frame : "never"}`).join("; "));
  await page.screenshot({ path: "/tmp/live-2-switch.png" });

  // ---- check 1: reload from cache with the bridge paused ----
  if (!process.env.SKIP_RELOAD) {
  docker("docker pause deploy-bridge-1");
  try {
    await page.evaluate((h) => { location.hash = h; }, route(A));
    await page.reload({ waitUntil: "commit" });
    const r = await sampleUntil({ names: { text: A.name }, bubbles: { sel: ".rail-strip .rail-bubble-label" }, thread: { sel: ".thread-items [data-sequence]" } }, 8000);
    const nav = await page.evaluate(() => Math.round(performance.now()));
    const gateSeen = r.first.gated || r.gatedNow || /Loading your devices|behind|Connecting/i.test(r.first.h1 + r.h1Now);
    note("reload with the bridge paused paints workspaces from cache", Boolean(r.seen.names) && !gateSeen, `names ${r.seen.names ? r.seen.names.ms + "ms/frame" + r.seen.names.frame : "never"} (page age ${nav} ms), bubbles ${r.seen.bubbles ? r.seen.bubbles.ms + "ms" : "never"}, thread ${r.seen.thread ? r.seen.thread.ms + "ms" : "never"}, gated first=${r.first.gated} now=${r.gatedNow}, h1="${r.first.h1 || r.h1Now}"`);
    await page.screenshot({ path: "/tmp/live-1-reload-paused.png" });
  } finally {
    docker("docker unpause deploy-bridge-1");
  }
  // Let the session come back before the write checks.
  await page.waitForTimeout(6000);
  }

  // ---- check 3a: a write in the ACTIVE workspace reaches its git surface ----
  await page.evaluate((h) => { location.hash = h; }, route(A, "changes"));
  await page.waitForSelector(".gitpane", { timeout: 30000 });
  await page.waitForTimeout(1000);
  const markerA = `live-active-${Date.now().toString(36)}.txt`;
  docker(`docker exec deploy-bridge-1 sh -c "echo hello > ${A.gitDir}/${markerA}"`);
  const raMs = await pollText(markerA, 15000);
  const ra = { file: raMs === null ? null : { ms: raMs } };
  const statusHasA = await page.evaluate(({ entity, name }) => new Promise((resolve) => { const req = indexedDB.open("build-cache"); req.onsuccess = () => { const store = req.result.transaction("records", "readonly").objectStore("records"); const all = store.getAll(); const keys = store.getAllKeys(); all.onsuccess = () => { keys.onsuccess = () => { let hit = false; keys.result.forEach((k, i) => { const key = String(k); if (key.includes(entity) && key.split("|")[2] === "status" && (all.result[i]?.value?.files || []).some((f) => f.path === name)) hit = true; }); resolve(hit); }; }; }; }), { entity: A.entityId.slice(0, 16), name: markerA });
  const bodyHasA = (await page.textContent("body")).includes(markerA);
  note("active workspace: a new file shows in the git surface within ~1 s", Boolean(ra.file) && ra.file.ms <= 2500, ra.file ? `${ra.file.ms} ms` : `never within 15 s (record has it: ${statusHasA}, body text has it now: ${bodyHasA}, hash ${await page.evaluate(() => location.hash)})`);
  await page.screenshot({ path: "/tmp/live-3a-active-write.png" });

  // ---- check 3b: a write in a BACKGROUND workspace moves its inbox row within 30 s ----
  const rowText = (name) => page.evaluate((n) => {
    const el = [...document.querySelectorAll("*")].find((e) => e.children.length < 12 && e.textContent.includes(n) && e.textContent.length < 400);
    return el ? el.textContent.replace(/\s+/g, " ").trim() : null;
  }, name);
  const before = await rowText(B.name);
  const markerB = `live-bg-${Date.now().toString(36)}.txt`;
  const tB = Date.now();
  docker(`docker exec deploy-bridge-1 sh -c "echo hello > ${B.gitDir}/${markerB}"`);
  const statusHas = (entity, name) => page.evaluate(({ entity, name }) => new Promise((resolve) => { const req = indexedDB.open("build-cache"); req.onsuccess = () => { const store = req.result.transaction("records", "readonly").objectStore("records"); const all = store.getAll(); const keys = store.getAllKeys(); all.onsuccess = () => { keys.onsuccess = () => { let hit = false; keys.result.forEach((k, i) => { const key = String(k); if (key.includes(entity) && key.split("|")[2] === "status" && (all.result[i]?.value?.files || []).some((f) => f.path === name)) hit = true; }); resolve(hit); }; }; }; }), { entity, name });
  let after = before; let moved = false; let recordAt = null;
  while (Date.now() - tB < 40000 && !(moved && recordAt)) {
    await page.waitForTimeout(500);
    if (!recordAt && (await statusHas(B.entityId.slice(0, 16), markerB))) recordAt = Date.now() - tB;
    if (!moved) { after = await rowText(B.name); if (after !== before) moved = true; }
  }
  note("background workspace: its cached git status holds the file within 30 s", recordAt !== null && recordAt <= 32000, recordAt !== null ? `${recordAt} ms` : "never within 40 s");
  note("background workspace: its inbox row moves within 30 s of a write", moved && Date.now() - tB <= 32000, moved ? `${Date.now() - tB} ms: "${before}" → "${after}"` : `no change in 40 s: "${before}"`);
  // Then open it: the git surface must already hold the file, no wait.
  await page.evaluate((h) => { location.hash = h; }, route(B, "changes"));
  const rbMs = await pollText(markerB, 5000);
  note("opening the background workspace shows the file from cache", rbMs !== null, rbMs !== null ? `${rbMs} ms` : "never within 5 s");
  await page.screenshot({ path: "/tmp/live-3b-background-write.png" });
} catch (e) {
  note("live check threw", false, e.message);
  await page.screenshot({ path: "/tmp/live-error.png" }).catch(() => {});
} finally {
  writeFileSync("/tmp/live-results.json", JSON.stringify(results, null, 2));
  await browser.close();
}
console.log(results.every((r) => r.ok) ? "LIVE PASS" : "LIVE FAIL");
process.exit(results.every((r) => r.ok) ? 0 : 1);
