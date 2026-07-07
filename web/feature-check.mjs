// Feature regression for the w1/w2 hardening batch — drives the REAL web app
// (compose stack: app :8090, relay :18090, QA bridge) and asserts the batch's
// cross-stream contracts end-to-end:
//
//   a. root redirect        GET / → 302/303 /app/
//   b. task.abandon         abandon a live task from the UI; worktree pruned
//   c. task.delete          delete a terminal task; gone from the board + reload
//   d. merge honesty        a conflicting merge is REFUSED — never shows MERGED,
//                           the task view surfaces the merge_failed reason
//   f. offline error state  stop the bridge → explicit offline banner, not blank;
//                           start it → silent recovery
//   g. terminal resize      the drawer terminal refits cols/rows to the viewport
//   h. push payload shape   /api/push/notify enforces the content-free contract
//                           (kind allowlist, device-auth, no goal/title field)
//
// Crash detection (contract #5) is intentionally NOT covered here — the scripted
// QA agent advances planning/building synchronously, so a task never lingers in
// a phase with a live-but-killable harness to pkill. It is covered by the bridge
// unit test app.rs::harness_exit_promotes_to_idle_unreported_within_seconds.
//
// Prereqs: the compose stack up, bridge paired to QA_EMAIL (default demo@localhost):
//   API_URL=http://localhost:8090 QA_EMAIL=demo@localhost PAIRING_CODE=COMPOSE-PAIR node pair.mjs
//
// Usage: APP=http://localhost:8090 node feature-check.mjs

import { chromium } from "playwright";
import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";

const APP = process.env.APP || "http://localhost:8090";
const EMAIL = process.env.QA_EMAIL || "demo@localhost";
const BRIDGE_CONTAINER = process.env.BRIDGE_CONTAINER || "deploy_bridge_1";
const WORKTREES_DIR = process.env.BRIDGE_WORKTREES_DIR || "/worktrees/proj-1";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let fail = 0;
const check = (name, ok, detail = "") => {
  console.log(`${ok ? "✓" : "✗"} ${name}${detail ? "  — " + detail : ""}`);
  if (!ok) fail++;
};
const section = async (label, fn) => {
  try {
    await fn();
  } catch (e) {
    check(`${label} (threw)`, false, e.message);
    await page.screenshot({ path: `/tmp/feature-check-${label}.png` }).catch(() => {});
  }
};

/** List the bridge's task worktree directories (podman exec into the container). */
function listWorktrees() {
  try {
    const out = execFileSync(
      "podman",
      ["exec", BRIDGE_CONTAINER, "sh", "-c", `ls -1 ${WORKTREES_DIR} 2>/dev/null || true`],
      { encoding: "utf8" },
    );
    return out.split("\n").map((s) => s.trim()).filter((s) => s && !s.endsWith(".sock"));
  } catch {
    return [];
  }
}

const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 1200, height: 860 } });
const page = await context.newPage();
page.on("console", (m) => {
  if (m.type() === "error") console.log("[page]", m.text());
});
// The abandon flow uses window.confirm — always accept.
page.on("dialog", (d) => d.accept().catch(() => {}));

async function connect() {
  await page.goto(`${APP}/auth/dummy/login`, { waitUntil: "load" });
  await page.fill('input[name="email"]', EMAIL);
  await page.fill('input[name="name"]', "Feature QA");
  await Promise.all([
    page.waitForNavigation({ waitUntil: "load", timeout: 15000 }).catch(() => {}),
    page.evaluate(() => document.querySelector('form[action="/auth/dummy-login"]').submit()),
  ]);
  await page.goto(`${APP}/app/`, { waitUntil: "load" });
  await page.waitForFunction(() => document.getElementById("conn")?.textContent?.includes("connected"), null, { timeout: 30000 });
}

/** Dispatch a standard task; returns its task id once the plan is ready to review. */
async function dispatchStandardTask(goal) {
  // Defensive: a prior section may have left the terminal drawer open, whose
  // canvas would intercept clicks on the board.
  await page.evaluate(() => document.getElementById("drawer")?.classList.remove("show")).catch(() => {});
  await page.goto(`${APP}/app/#/`, { waitUntil: "load" }).catch(() => {});
  await page.waitForSelector("#newtask", { timeout: 15000 });
  await page.click("#newtask");
  await page.waitForSelector("#goal", { timeout: 5000 });
  // The project picker fills async from the bridge; wait for a real option.
  await page.waitForFunction(() => {
    const s = document.querySelector("#project");
    return s && s.options.length && s.value;
  }, null, { timeout: 10000 });
  await page.fill("#goal", goal);
  await page.click("#dispatch");
  await page.waitForFunction(() => location.hash.includes("/task/"), null, { timeout: 15000 });
  const id = await page.evaluate(() => decodeURIComponent(location.hash.split("/task/")[1].split("/")[0]));
  await page.waitForSelector("#approvePlan", { timeout: 60000 });
  return id;
}

/** From a plan_review task, approve the plan and wait for the diff review gate. */
async function approvePlanToReview(id) {
  await page.goto(`${APP}/app/#/task/${encodeURIComponent(id)}/plan`, { waitUntil: "load" });
  await page.waitForSelector("#approvePlan", { timeout: 60000 });
  await page.click("#approvePlan");
  await page.goto(`${APP}/app/#/task/${encodeURIComponent(id)}/diff`, { waitUntil: "load" }).catch(() => {});
  await page.waitForSelector("#gitprimary", { timeout: 120000 });
}

let abandonedTaskId = null;

// ---- a. Root redirect ------------------------------------------------------
await section("root-redirect", async () => {
  const res = await fetch(`${APP}/`, { redirect: "manual" });
  const location = res.headers.get("location") || "";
  check(
    "GET / redirects (302/303) to /app/",
    (res.status === 302 || res.status === 303) && location.includes("/app/"),
    `status ${res.status} location ${location}`,
  );
});

await connect();
check("login + live E2EE session", true);

// ---- b. Abandon a live task; worktree pruned -------------------------------
await section("abandon", async () => {
  const id = await dispatchStandardTask(`feature-abandon a task ${Date.now()}`);
  abandonedTaskId = id;
  const before = listWorktrees();
  await page.waitForSelector("#abandonTask", { timeout: 5000 });
  await page.click("#abandonTask"); // dialog auto-accepted
  // The state flips to abandoned: the task view now offers Delete (terminal), and
  // the chip reads ABANDONED.
  await page.waitForFunction(
    () => /ABANDONED/.test(document.querySelector(".thead .chip")?.textContent || "") && !!document.querySelector("#deleteTask"),
    null,
    { timeout: 15000 },
  );
  check("task view shows ABANDONED + Delete action after abandon", true);

  const after = listWorktrees();
  check(
    "bridge pruned the abandoned task's worktree",
    after.length < before.length,
    `worktrees ${before.length} → ${after.length}`,
  );

  // Board reflects it: the abandoned task lands in DONE with an ABANDONED chip.
  await page.goto(`${APP}/app/#/`, { waitUntil: "load" });
  await page.waitForSelector(`.card[data-id="${id}"]`, { timeout: 10000 });
  const chip = await page.$eval(`.card[data-id="${id}"] .chip`, (el) => el.textContent.trim());
  check("board shows the task as ABANDONED", chip === "ABANDONED", `chip=${chip}`);
});

// ---- c. Delete a terminal task; gone from board + reload -------------------
await section("delete", async () => {
  const id = abandonedTaskId;
  if (!id) throw new Error("no abandoned task to delete (abandon section failed)");
  await page.goto(`${APP}/app/#/task/${encodeURIComponent(id)}/plan`, { waitUntil: "load" });
  await page.waitForSelector("#deleteTask", { timeout: 10000 });
  await page.click("#deleteTask");
  // Delete routes back to the board.
  await page.waitForFunction(() => !location.hash.includes("/task/"), null, { timeout: 10000 });
  await page.waitForSelector("#newtask", { timeout: 10000 });
  await sleep(500);
  check("deleted task is gone from the board", !(await page.$(`.card[data-id="${id}"]`)));

  // Still gone after a full reload (durably deleted from the task store).
  await page.reload({ waitUntil: "load" });
  await page.waitForFunction(() => document.getElementById("conn")?.textContent?.includes("connected"), null, { timeout: 30000 });
  await page.waitForSelector("#newtask", { timeout: 10000 });
  await sleep(1000);
  check("deleted task stays gone after reload", !(await page.$(`.card[data-id="${id}"]`)));
});

// ---- d. Merge honesty: a conflicting merge is refused ----------------------
await section("merge-honesty", async () => {
  // Both tasks branch from the SAME main HEAD and write the SAME file (the QA
  // agent always writes result.txt), so merging the first lands cleanly and the
  // second then conflicts on result.txt.
  const stamp = Date.now();
  const idA = await dispatchStandardTask(`feature-merge first writer ${stamp}`);
  await approvePlanToReview(idA);
  const idB = await dispatchStandardTask(`feature-merge second writer ${stamp}`);
  await approvePlanToReview(idB);

  // Merge A — clean, lands on the board as MERGED.
  await page.goto(`${APP}/app/#/task/${encodeURIComponent(idA)}/diff`, { waitUntil: "load" });
  await page.waitForSelector("#gitprimary", { timeout: 60000 });
  await page.click("#gitprimary");
  await page.waitForFunction(() => !location.hash.includes("/task/"), null, { timeout: 30000 });
  await page.waitForFunction(
    (id) => document.querySelector(`.card[data-id="${id}"] .chip`)?.textContent.trim() === "MERGED",
    idA,
    { timeout: 30000 },
  );
  check("first task merges cleanly (board shows MERGED)", true);

  // Merge B — conflicts. It must NOT show MERGED; the task view must surface the
  // merge_failed reason (contract #2).
  await page.goto(`${APP}/app/#/task/${encodeURIComponent(idB)}/diff`, { waitUntil: "load" });
  await page.waitForSelector("#gitprimary", { timeout: 60000 });
  await page.click("#gitprimary");
  // The failed merge keeps us on the task view (no navigation to the board) and
  // the last_error banner appears on the next poll.
  await page.waitForFunction(
    () => {
      const banner = document.querySelector("#taskError");
      return banner && !banner.hidden && /merge_failed/i.test(banner.textContent);
    },
    null,
    { timeout: 15000 },
  );
  const bannerText = await page.$eval("#taskError", (el) => el.textContent);
  check("conflicting task shows the merge_failed banner with a reason", /merge_failed:/i.test(bannerText) && bannerText.length > "merge_failed:".length + 3, bannerText.slice(0, 90));
  check("conflicting merge did NOT navigate to the board", await page.evaluate((id) => location.hash.includes(`/task/${id}`), idB));

  // Board confirms B is not merged and carries the error.
  await page.goto(`${APP}/app/#/`, { waitUntil: "load" });
  await page.waitForSelector(`.card[data-id="${idB}"]`, { timeout: 10000 });
  const chipB = await page.$eval(`.card[data-id="${idB}"] .chip`, (el) => el.textContent.trim());
  check("board does NOT show the conflicting task as MERGED", chipB !== "MERGED", `chip=${chipB}`);
  const hasErr = await page.$(`.card[data-id="${idB}"] .cerr`);
  check("board card surfaces the merge error", !!hasErr);
});

// ---- g. Terminal drawer refits cols/rows to the viewport -------------------
await section("terminal-resize", async () => {
  // Open a fresh task's view so the terminal drawer has a host page.
  const id = await dispatchStandardTask(`feature-terminal drawer ${Date.now()}`);
  await page.goto(`${APP}/app/#/task/${encodeURIComponent(id)}/plan`, { waitUntil: "load" });
  await page.setViewportSize({ width: 1200, height: 860 });
  await page.waitForSelector("#termToggle", { timeout: 10000 });
  await page.click("#termToggle");
  await page.waitForSelector("#drawer.show", { timeout: 10000 });
  // The drawer boots ghostty-web (wasm) lazily and exposes the instance for QA.
  await page.waitForFunction(() => window.__buildTerminal && window.__buildTerminal.cols > 0, null, { timeout: 30000 });
  const wide = await page.evaluate(() => ({ cols: window.__buildTerminal.cols, rows: window.__buildTerminal.rows }));
  check("terminal reports a live cols/rows once booted", wide.cols > 0 && wide.rows > 0, `cols=${wide.cols} rows=${wide.rows}`);

  // Shrink the viewport; the terminal must refit to fewer columns.
  await page.setViewportSize({ width: 480, height: 720 });
  await page.waitForFunction(
    (was) => window.__buildTerminal && window.__buildTerminal.cols !== was,
    wide.cols,
    { timeout: 10000 },
  );
  const narrow = await page.evaluate(() => ({ cols: window.__buildTerminal.cols, rows: window.__buildTerminal.rows }));
  check("terminal cols refit when the viewport shrank", narrow.cols !== wide.cols && narrow.cols < wide.cols, `${wide.cols} → ${narrow.cols} cols`);

  // And grow it back — cols increase again (the refit is bidirectional).
  await page.setViewportSize({ width: 1200, height: 860 });
  await page.waitForFunction(
    (was) => window.__buildTerminal && window.__buildTerminal.cols > was,
    narrow.cols,
    { timeout: 10000 },
  );
  check("terminal cols grow again when the viewport grows", true);
  await page.click("#dx").catch(() => {});
});

// ---- h. Push payload shape (content-free contract #6) ----------------------
await section("push-payload", async () => {
  const notify = (body) =>
    fetch(`${APP}/api/push/notify`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });

  // Unknown kind is rejected by the allowlist (a kind is a generic status label,
  // never task content) — checked before any device lookup.
  const bad = await notify({ device_id: randomUUID(), task_id: "t-1", kind: "leak-the-goal", timestamp: Math.floor(Date.now() / 1000), signature_b64: "x" });
  check("notify rejects an unknown kind (allowlist enforced)", bad.status === 400, `status ${bad.status}`);

  // A valid kind still requires device-auth — an unknown device is refused
  // (401/403), never a 500, and the request body has no field for a goal/title.
  const unauth = await notify({ device_id: randomUUID(), task_id: "t-1", kind: "attention", timestamp: Math.floor(Date.now() / 1000), signature_b64: "not-a-real-signature" });
  check("notify enforces device signature auth for a valid kind", unauth.status === 401 || unauth.status === 403, `status ${unauth.status}`);

  // The notify body accepts only {device_id, task_id, kind, timestamp,
  // signature_b64}; a smuggled goal/title is simply ignored (still 400 on kind),
  // so task content structurally cannot reach the server.
  const smuggled = await notify({ device_id: randomUUID(), task_id: "t-1", kind: "leak", timestamp: Math.floor(Date.now() / 1000), signature_b64: "x", goal: "SECRET GOAL TEXT", title: "SECRET TITLE" });
  check("notify ignores smuggled goal/title fields (content-free by shape)", smuggled.status === 400, `status ${smuggled.status}`);
});

// ---- f. Offline error state (stop the bridge, then recover) ----------------
// Last, because it briefly takes the bridge down.
await section("offline-recovery", async () => {
  const id = await dispatchStandardTask(`feature-offline view ${Date.now()}`);
  await page.goto(`${APP}/app/#/task/${encodeURIComponent(id)}/plan`, { waitUntil: "load" });
  await page.waitForSelector("#approvePlan", { timeout: 60000 });

  execFileSync("podman", ["stop", "-t", "3", BRIDGE_CONTAINER], { encoding: "utf8" });
  // The relay pushes device_offline within seconds → explicit offline banner,
  // not a silently-empty view.
  await page.waitForFunction(
    () => document.body.classList.contains("offline") && !document.getElementById("offbar")?.hidden,
    null,
    { timeout: 20000 },
  );
  check("bridge offline shows the explicit offline banner (not blank)", true);
  const connText = await page.$eval("#conn", (el) => el.textContent);
  check("connection indicator reflects reconnecting", /reconnect/i.test(connText), connText.trim());

  execFileSync("podman", ["start", BRIDGE_CONTAINER], { encoding: "utf8" });
  // Silent recovery: the banner clears and the session reconnects on its own.
  await page.waitForFunction(
    () => !document.body.classList.contains("offline") && document.getElementById("conn")?.textContent?.includes("connected"),
    null,
    { timeout: 60000 },
  );
  check("bridge return recovers the session silently", true);
});

await browser.close();
console.log(fail === 0 ? "FEATURE CHECK PASS" : `FEATURE CHECK FAIL (${fail})`);
process.exit(fail === 0 ? 0 : 1);
