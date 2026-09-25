// #169 in real WebKit: the local cache keeps answering when WebKit's storage
// process dies under it, the way a suspended iOS page loses its IndexedDB.
//
//   echo "docker run --rm --ipc=host -v $PWD:/repo:ro mcr.microsoft.com/playwright:v1.63.0-noble \
//     bash -c 'cd /tmp && npm i --silent playwright@1.63.0 && cp /repo/web/webkit-cache-storage-loss.mjs . \
//     && node webkit-cache-storage-loss.mjs /repo/spa/src/core'" | newgrp docker
//
// Playwright's WebKit runs only in that image on this machine (host WebKit
// builds miss their ICU). Exits non-zero when a read answered nothing for a
// record still on disk, or the cache is not ready afterwards.
//
// # What it does
//
// Serves the cache module and its imports from the directory given, over an
// on-disk profile (an ephemeral one keeps IndexedDB inside the network process,
// so killing it would take the records too). A loop keeps reads and atomic
// merges in flight; the WPE network process — where WebKit's IndexedDB server
// lives — is SIGKILLed under them, and WebKit relaunches it on the next use.
// The transactions in flight fail with `UnknownError: An internal error was
// encountered in the Indexed Database server`, which the cache before #169 did
// not recognise as a lost connection: it stood down at once, and ~370 of ~400
// reads answered nothing for a record that was on disk the whole time.
//
// # What only a phone can show
//
// A suspension is not a crash: iOS freezes the page and WebKit closes its
// connection, and the errors after a resume (`Connection to Indexed Database
// server lost`, opens that fail or never answer) come from that path, which no
// desktop WebKit reproduces. Settings → Diagnostics records every `local-cache`
// event with the page's visibility and time since it was shown, for that.

import { webkit } from "playwright";
import { mkdtempSync, readFileSync } from "node:fs";
import { execSync } from "node:child_process";

const moduleDir = process.argv[2] || "/repo/spa/src/core";
const address = { deviceId: "dev-1", entityId: "run-1", kind: "status" };

const networkProcesses = () =>
  execSync("ps -eo pid,args | grep -i networkprocess | grep -v grep || true").toString().trim().split("\n").filter(Boolean);

const context = await webkit.launchPersistentContext(mkdtempSync("/tmp/profile-"));
const page = await context.newPage();
page.on("console", (message) => console.log(`[console.${message.type()}] ${message.text().slice(0, 240)}`));
await page.route("http://test.local/**", (route) => {
  const path = new URL(route.request().url()).pathname;
  if (path === "/") return route.fulfill({ contentType: "text/html", body: "<!doctype html><title>cache</title>" });
  return route.fulfill({ contentType: "text/javascript", body: readFileSync(`${moduleDir}${path}`, "utf8") });
});
await page.goto("http://test.local/");

await page.evaluate(async (address) => {
  window.cache = await import("/localCache.js");
  window.diagnostics = await import("/connectionDiagnostics.js");
  await window.cache.writeCached(address, { head: "before-sleep" });
  window.storm = { running: true, reads: 0, emptyReads: 0, writes: 0, failedWrites: 0 };
  window.stormDone = (async () => {
    for (let n = 1; window.storm.running; n += 1) {
      await new Promise((resolve) => setTimeout(resolve, 5));
      const other = { deviceId: "dev-1", entityId: "run-2", kind: "status", sub: String(n % 20) };
      const [record, wrote] = await Promise.all([
        window.cache.readCached(address),
        window.cache.mergeCachedAtomically(other, () => ({ n })),
      ]);
      window.storm.reads += 1;
      if (!record) window.storm.emptyReads += 1;
      if (wrote) window.storm.writes += 1;
      else window.storm.failedWrites += 1;
    }
  })();
}, address);

await new Promise((resolve) => setTimeout(resolve, 300));
for (const line of networkProcesses()) process.kill(Number(line.trim().split(/\s+/)[0]), "SIGKILL");
console.log("killed the network process");
await new Promise((resolve) => setTimeout(resolve, 3000));

const after = await page.evaluate(async (address) => {
  window.storm.running = false;
  await window.stormDone;
  const record = await window.cache.readCached(address);
  return {
    storm: window.storm,
    readAfter: record?.value ?? null,
    health: window.cache.cacheHealth?.() ?? null,
    events: window.diagnostics.connectionDiagnosticHistory()
      .filter((entry) => entry.connection === "local-cache")
      .map((entry) => `${entry.event} ${entry.error || ""} ${entry.message || ""}`.trim()),
  };
}, address);
console.log(JSON.stringify(after, null, 1));
await context.close();

const failures = [];
if (after.storm.emptyReads) failures.push(`${after.storm.emptyReads} reads answered nothing for a stored record`);
if (after.readAfter?.head !== "before-sleep") failures.push("the record did not read back afterwards");
if (after.health?.state !== "ready") failures.push(`the cache is ${after.health?.state ?? "without a health report"}`);
if (failures.length) {
  console.error(`FAIL: ${failures.join("; ")}`);
  process.exit(1);
}
console.log("PASS: the cache rode out the storage process dying");
