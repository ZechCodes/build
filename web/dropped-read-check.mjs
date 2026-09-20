// #24 on a real stack: a read that fails because the session dropped keeps the
// cached copy quietly, marks itself, and reads again on reconnect.
//
//   ISSUES_REPO=<this checkout> node web/dropped-read-check.mjs
//
// Reads /tmp/live-seed.json, so run web/live-seed.mjs first and write its SEED
// line there. Exits non-zero on a failed check: a regression harness that
// always exits 0 is one nobody notices failing.
//
// # Why this exists as a script and not only as a unit test
//
// The unit tests cover the policy over injected failures and they all passed
// against a build where, on a live stack, the mark never appeared and the
// retry never armed. What they could not know is the ORDER real events arrive
// in: a call dies on the wire seconds before ICE gives up and the recovery
// supervisor starts reconnecting. Asking "is this machine away?" at the
// instant a read fails is asking too early, and believing the "no" is how the
// whole visible half of #24 went silently missing. Only a real stack has that
// gap in it.
//
// # Two things that produced a false result first
//
//   • PAUSE, never stop. `docker stop` deregisters the device: the account
//     stops listing it online, the supervisor stands down, and the surface is
//     then a machine that is GONE — which core/deviceNotice.js names, and the
//     mark is correctly silent. Zech's phone is the other case entirely: it
//     stays online and registered while its session dies at the network layer.
//     A paused container is that — holding its registration, answering
//     nothing.
//   • WAIT past the path deadline. A read that has not timed out has not
//     failed, and watching a still-pending read proves nothing. The deadline
//     is read out of the SPA below rather than written here twice.
//
// # Why the timeline count, and not "the page still has content"
//
// A cached copy also has content, so "something is on screen" cannot tell a
// retry that landed from a mark that merely cleared over a copy that never
// moved. So a comment is added with NO page open on the issue: nothing can
// deliver it by push, and the per-issue cache record — written only by the
// issue page itself — still holds the shorter timeline. A count that grows
// after the reconnect can only have come from a read that landed.

import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { execSync } from "node:child_process";
import { chromium } from "/home/zech/.local/share/mise/installs/npm-playwright/1.63.0/node_modules/playwright/index.mjs";

const APP = process.env.APP_URL || "http://localhost:8090";
const BRIDGE = process.env.BRIDGE_CONTAINER || "deploy-bridge-1";

/** The ONE checkout this runs off — refused rather than defaulted, for the
 *  same reason web/tracker-check.mjs refuses it: there is more than one
 *  build-web checkout on this machine, and a forgotten variable would point
 *  the run at another tree's compose file and pass against a bridge that was
 *  never under test. */
const REPO = process.env.ISSUES_REPO;
if (!REPO) {
  console.error("set ISSUES_REPO to the checkout under test — this run must not mix checkouts");
  process.exit(2);
}
const compose = `${REPO}/deploy/compose.real.yml`;
if (!existsSync(compose)) {
  console.error(`no compose file at ${compose} — is ISSUES_REPO a build-web checkout?`);
  process.exit(2);
}

/** The SPA's own path deadline, read from the module that arms it, so this
 *  script cannot drift from the number it is waiting out. */
const PATH_DEADLINE_MS = (() => {
  const source = readFileSync(`${REPO}/spa/src/core/sessionRpc.js`, "utf8");
  const found = source.match(/DEFAULT_RPC_TIMEOUT_MS\s*=\s*(\d+)/);
  if (!found) throw new Error("no DEFAULT_RPC_TIMEOUT_MS in core/sessionRpc.js — this check needs it");
  return Number(found[1]);
})();

const seed = JSON.parse(readFileSync("/tmp/live-seed.json", "utf8"));
const docker = (command) =>
  execSync(`echo '${command}' | newgrp docker 2>&1`, { encoding: "utf8", shell: "/bin/bash" });

const results = [];
const record = (name, ok, detail = "") => {
  results.push({ name, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
};

/** One bridge RPC from inside the qa container, which is where the compose
 *  network and the harness deps are. */
function call(method, params) {
  const script = `import * as transport from "@build/secure-transport"; import { openDeviceLink, openRendezvous } from "./client.mjs"; import { loginWithDummy } from "./skrift-auth.mjs";
const { cookie, mintGatewayToken } = await loginWithDummy(process.env.API_URL, { email: "qa@localhost" });
const rendezvous = await openRendezvous({ relayUrl: process.env.RELAY_URL, mintGatewayToken });
const link = await openDeviceLink({ rendezvous, transport, apiUrl: process.env.API_URL, cookie });
const r = await link.session.call(${JSON.stringify(method)}, ${JSON.stringify(params)});
console.log("RESULT " + JSON.stringify(r)); process.exit(0);`;
  writeFileSync("/tmp/dropped-read-call.mjs", script);
  const out = execSync(
    `echo 'docker compose -f ${compose} --profile qa run --rm -T --no-deps qa node --input-type=module - < /tmp/dropped-read-call.mjs' | newgrp docker 2>&1`,
    { encoding: "utf8", shell: "/bin/bash" },
  );
  const line = out.split("\n").find((one) => one.startsWith("RESULT "));
  if (!line) throw new Error(`no result from ${method}: ${out.trim().slice(-300)}`);
  return JSON.parse(line.slice("RESULT ".length));
}

const browser = await chromium.launch({ executablePath: "/usr/bin/chromium", headless: true, args: ["--no-sandbox"] });
// One explicit context: browser.newPage() wraps a context of its own with no
// session cookie, and a page without it lands on the sign-in gate — which
// reads exactly like a broken surface.
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await context.newPage();

await page.goto(`${APP}/auth/dummy/login`, { waitUntil: "load" });
await page.fill('input[name="email"]', "qa@localhost");
await Promise.all([
  page.waitForNavigation({ waitUntil: "load" }),
  page.evaluate(() => document.querySelector('form[action="/auth/dummy-login"]').submit()),
]);

const stamp = Date.now().toString(36);
const filed = call("issues.create", {
  project_id: seed.projectId,
  title: `dropped-read-${stamp}`,
  body: "Filed by web/dropped-read-check.mjs.",
});
const issueId = filed.issue.id;
const issuesTab = `${APP}/app/#/device/${seed.deviceId}/project/${seed.projectId}/issues`;
const issuePage = `${issuesTab}/${issueId}`;

const entries = () => page.evaluate(() => document.querySelectorAll(".issue-entry").length);
const mark = () => page.evaluate(() => document.querySelector(".read-wait")?.textContent ?? null);
const toasts = () => page.evaluate(() => window.__toasts || []);
const watchToasts = () =>
  page.evaluate(() => {
    window.__toasts = [];
    new MutationObserver((records) => {
      for (const one of records)
        for (const node of one.addedNodes)
          if (node.nodeType === 1 && (node.className || "").toString().includes("toast"))
            window.__toasts.push(node.innerText.replace(/\s+/g, " ").trim());
    }).observe(document.body, { childList: true, subtree: true });
  });

// ── 1. read it once with the bridge answering, so the cache is warm ─────────
await page.goto(issuePage, { waitUntil: "load" });
await page.waitForTimeout(8000);
const cached = await entries();
record("the issue page reads and paints with the bridge answering", cached > 0, `${cached} timeline entr${cached === 1 ? "y" : "ies"} cached`);

// ── 2. move a comment onto it with no page open to hear about it ────────────
await page.goto(issuesTab, { waitUntil: "load" });
await page.waitForTimeout(3000);
call("issues.comment", { issue_id: issueId, body: `Written off-page by dropped-read-check ${stamp}.` });

// ── 3. the session dies under the page ──────────────────────────────────────
await watchToasts();
console.log(`\npausing ${BRIDGE} — registered, answering nothing\n`);
docker(`docker pause ${BRIDGE}`);
const pausedAt = Date.now();
await page.goto(issuePage, { waitUntil: "load" });

let markedAt = null;
let markText = null;
for (let step = 0; step < 16; step++) {
  await page.waitForTimeout(3000);
  const shown = await mark();
  if (shown && markedAt === null) {
    markedAt = Date.now() - pausedAt;
    markText = shown;
  }
  if (step % 3 === 0) console.log(`   +${Math.round((Date.now() - pausedAt) / 1000)}s  mark=${JSON.stringify(shown)}  toasts=${(await toasts()).length}`);
}

const duringCount = await entries();
record(
  "the page keeps the copy it had, and not the comment it never read",
  duringCount === cached,
  `${duringCount} entries on screen, cached ${cached} — the off-page comment is not among them`,
);
record("…and raises no toast about it", (await toasts()).length === 0, JSON.stringify(await toasts()).slice(0, 200));
record("the surface is marked while the machine is being reconnected to", markText !== null, markText ? `${JSON.stringify(markText)} at +${Math.round(markedAt / 1000)}s` : "never marked");
// The mark cannot honestly appear before the read it is about has failed.
record(
  `the mark waits out the path deadline first (${PATH_DEADLINE_MS} ms)`,
  markedAt !== null && markedAt >= PATH_DEADLINE_MS,
  markedAt === null ? "never marked" : `marked at ${markedAt} ms`,
);

// ── 4. the machine comes back ───────────────────────────────────────────────
console.log(`\nunpausing ${BRIDGE}\n`);
docker(`docker unpause ${BRIDGE}`);
const resumedAt = Date.now();

let grewAt = null;
for (let step = 0; step < 40; step++) {
  await page.waitForTimeout(2000);
  if ((await entries()) > duringCount) {
    grewAt = Date.now() - resumedAt;
    break;
  }
}
const finalCount = await entries();
record(
  "the retry re-reads on reconnect — the off-page comment arrives",
  grewAt !== null,
  `${duringCount} → ${finalCount} entries${grewAt === null ? " (never grew in 80s)" : ` at +${Math.round(grewAt / 1000)}s`}`,
);
record("the mark clears with it", (await mark()) === null, JSON.stringify(await mark()));
record("no toast across the whole drop", (await toasts()).length === 0, JSON.stringify(await toasts()).slice(0, 250));

await browser.close();
console.log("\n──────── summary ────────");
for (const { name, ok, detail } of results) console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
const passed = results.filter((one) => one.ok).length;
console.log(`\n${passed}/${results.length} passed`);
process.exit(passed === results.length ? 0 : 1);
