import assert from "node:assert/strict";
import { readFile, rename, writeFile } from "node:fs/promises";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { createServer } from "../../spa/node_modules/vite/dist/node/index.js";
import { chromium } from "../../spa/node_modules/playwright-core/index.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const artifacts = process.argv[2];
const mode = process.env.BUILD_RTC_LAN_MODE || "delayed";
const startsDirect = ["early-unresolved", "far-edge-unresolved"].includes(mode);
const command = promisify(execFile);
const save = async (name, value) => {
  const target = path.join(artifacts, name);
  await writeFile(`${target}.tmp`, typeof value === "string" ? value : JSON.stringify(value, null, 2));
  await rename(`${target}.tmp`, target);
};
const server = await createServer({
  root: path.resolve(here, "../../spa"),
  configFile: false,
  server: { host: "127.0.0.1", port: 9001, strictPort: true, hmr: false, watch: null,
    fs: { allow: [path.resolve(here, "../../..")] } },
  plugins: [{ name: "lan-fixture", resolveId(id) {
    if (id === "/__lan_fixture__.mjs") return path.join(here, "browser.mjs");
  }, configureServer(vite) {
    vite.middlewares.use("/fixture", async (_request, response) => {
      response.setHeader("Content-Type", "text/html");
      response.end('<script type="module" src="/__lan_fixture__.mjs"></script>');
    });
  } }],
});
let browser;
let page;
const droppedHostChecks = async () => {
  const { stdout } = await command("nft", ["-j", "list", "table", "inet", "hold_checks"]);
  const rules = JSON.parse(stdout);
  await save("control-firewall.json", rules);
  return rules.nftables.flatMap((row) => row.rule?.expr || []).reduce((count, expression) => count + (expression.counter?.packets || 0), 0);
};
try {
  await server.listen();
  browser = await chromium.launch({ executablePath: "/usr/bin/chromium", headless: true,
    args: ["--no-sandbox", "--disable-dev-shm-usage"] });
  page = await browser.newPage();
  page.on("console", (event) => console.log(event.type(), event.text()));
  page.on("pageerror", (error) => console.error("page error", error));
  await page.exposeFunction("fixtureReady", () => save("browser-ready", "ready"));
  await page.exposeFunction("fixtureMode", () => mode);
  const initialCheckWindow = async (sourcePort, destinationPort) => {
    const wire = JSON.parse(await readFile(path.join(artifacts, "wire.json"), "utf8"));
    const checks = wire.browser_checks.filter((row) => row.source_port === sourcePort && row.destination_port === destinationPort);
    return { checks: checks.length, firstAt: checks[0]?.at, lastAt: checks.at(-1)?.at,
      observedAt: Date.now() / 1000 };
  };
  await page.exposeFunction("fixtureInitialCheckWindow", initialCheckWindow);
  await page.exposeFunction("fixtureInitialChecksExhausted", async (sourcePort, destinationPort) => {
    const window = await initialCheckWindow(sourcePort, destinationPort);
    // This installed Chromium sends 31 unanswered host checks across about
    // 15 seconds. Some runs retain their native in-progress stats row after
    // retransmission ends. Require the measured budget and age, then assert
    // retrospectively that no checks resumed before the actual restart.
    return window.checks >= 31 && window.observedAt - window.firstAt >= 15;
  });
  await page.exposeFunction("fixtureExhausted", (state) => save("exhausted.json", state));
  await page.exposeFunction("fixtureLatePrimed", (state) => save("late-primed.json", state));
  const gatheredHosts = [];
  await page.exposeFunction("fixtureHostPort", async (host) => {
    gatheredHosts.push(host);
    await save("gathered-hosts.json", gatheredHosts);
  });
  await page.exposeFunction("fixtureHost", async (host) => {
    await save("host.json", host);
    await save("gate", "ready");
    while (true) {
      try { await readFile(path.join(artifacts, "gated")); break; } catch (error) {
        if (error.code !== "ENOENT") throw error;
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
    }
  });
  await page.exposeFunction("fixtureRelease", async (state) => {
    console.log(startsDirect ? "initially direct" : "TURN carrying", JSON.stringify(state));
    await save("before.json", state);
    await save("release", "ready");
  });
  await page.exposeFunction("fixtureChecks", async () => {
    const wire = JSON.parse(await readFile(path.join(artifacts, "wire.json"), "utf8"));
    return wire.host_checks.filter((row) => row.after_release).length;
  });
  await page.exposeFunction("fixtureReleaseChecks", () => save("release-host-checks", "ready"));
  await page.exposeFunction("fixtureViable", (state) => {
    console.log("direct succeeded before restart", JSON.stringify(state));
    return save("viable.json", state);
  });
  await page.exposeFunction("fixtureDisconnectHost", async () => {
    await save("disconnect-host", "ready");
    const deadline = Date.now() + 3000;
    while (true) {
      try { await readFile(path.join(artifacts, "host-disconnected")); return; } catch (error) {
        if (error.code !== "ENOENT" || Date.now() >= deadline) throw error;
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
    }
  });
  await page.exposeFunction("fixtureBridgeFailed", async () =>
    /ice_state=failed\b/.test(await readFile(path.join(artifacts, "bridge.log"), "utf8")));
  await page.exposeFunction("fixtureSweepSkipped", async () =>
    /subnet-too-large/.test(await readFile(path.join(artifacts, "bridge.log"), "utf8")));
  await page.exposeFunction("fixtureSweepPrimed", async () => {
    const wire = JSON.parse(await readFile(path.join(artifacts, "wire.json"), "utf8"));
    const { before } = JSON.parse(await readFile(path.join(artifacts, "before.json"), "utf8"));
    const host = JSON.parse(await readFile(path.join(artifacts, "host.json"), "utf8"));
    return wire.host_socket_indications.some((row) => row.source_port === before.hostSocketPort && row.destination_port === host.port);
  });
  await page.exposeFunction("fixtureSweepExpired", async () =>
    /host-sweep .*"reason":"window-expired"/.test(await readFile(path.join(artifacts, "bridge.log"), "utf8")));
  await page.exposeFunction("fixtureDroppedHostChecks", droppedHostChecks);
  await page.goto("http://localhost:9001/fixture");
  console.log("fixture page loaded");
  await page.waitForFunction(() => typeof window.runLanUpgrade === "function");
  console.log("fixture modules ready");
  const deadline = Date.now() + 15000;
  while (true) {
    try { await readFile(path.join(artifacts, "firewall-ready")); break; } catch (error) {
      if (error.code !== "ENOENT" || Date.now() > deadline) throw error;
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
  }
  const result = await page.evaluate(() => window.runLanUpgrade());
  const bridgeLog = await readFile(path.join(artifacts, "bridge.log"), "utf8");
  const summaries = bridgeLog.split("\n").filter((line) => line.includes("remote_candidates "))
    .map((line) => JSON.parse(line.slice(line.indexOf("remote_candidates ") + "remote_candidates ".length)));
  const finalChecks = summaries.findLast((summary) => summary.ended === "close");
  const sweepEvents = bridgeLog.split("\n").filter((line) => line.includes("host-sweep "))
    .map((line) => ({ at: Number(line.match(/timestamp_ms=(\d+)/)?.[1]),
      ...JSON.parse(line.slice(line.indexOf("host-sweep ") + "host-sweep ".length)) }));
  const wire = JSON.parse(await readFile(path.join(artifacts, "wire.json"), "utf8"));
  if (mode === "delayed") {
    assert.equal(finalChecks?.reason, "direct-checks-succeeded", "actual ICE Failed must retain its successful host history at rtc.close");
    assert(finalChecks.hosts.some((host) => host.requests_sent > 0 && host.responses_received > 0 && host.succeeded),
      "the final host diagnostic retains real request/reply counters and success");
    assert(bridgeLog.indexOf("ice_state=failed") < bridgeLog.lastIndexOf("remote_candidates "),
      "the final host diagnostics follow actual bridge ICE Failed");
    assert(wire.host_checks.some((row) => row.after_release), "bridge sends outbound late-host STUN checks");
  } else if (["early-unresolved", "far-edge-unresolved", "unresolved", "late-unresolved"].includes(mode)) {
    assert(wire.mdns_silenced, "the responder remains permanently silenced");
    const indications = wire.host_socket_indications;
    assert(indications.length > 0, "bridge sends real host socket indications");
    assert(indications.every((row) => row.bytes === 28 && row.attributes === 1 && row.has_fingerprint && !row.has_username && !row.has_integrity),
      "the sweep payload carries no credential, username or attributes");
    assert(indications.some((row) => row.source_port === result.after.selected.remotePort),
      "the sweep uses the advertised host socket selected for direct");
    assert(wire.browser_checks.some((row) => row.at >= indications[0].at
      && row.destination_port === indications[0].source_port && row.has_integrity && row.has_username),
      "an authenticated browser host request follows the real-socket sweep");
    assert.match(bridgeLog, /carrying over host\/prflx candidates/, "the upgraded bridge pair is peer-reflexive");
    assert.match(bridgeLog, /host-sweep .*"prflx_followed":true/,
      "the sweep's generation records the authenticated peer-reflexive result");
    if (startsDirect) {
      assert.equal(result.after.restarts, 0, "the early sweep wins with no ICE restart");
      assert.deepEqual(result.appRpcPaths.map((row) => row.method),
        ["session.hello", "board.list", "project.list", "tasks.list", "ping"],
        "the selected pair is recorded before the first and every encrypted application RPC");
      assert(result.appRpcPaths.every((row) => row.path === "direct" && row.selected.localType !== "relay"
        && row.selected.remoteType !== "relay" && row.selected.id === result.before.selected.id),
        "the original direct pair carries every encrypted application RPC");
      const carrying = bridgeLog.split("\n").filter((line) => line.includes("carrying over "));
      assert(carrying.length > 0 && carrying.every((line) => line.includes("host/prflx candidates")),
        "the bridge never carried this application session on TURN");
      const startedAt = sweepEvents.find((event) => event.status === "started")?.at;
      const stopped = sweepEvents.findLast((event) => event.status === "stopped");
      const firstHitAt = indications[0].at * 1000;
      result.measurements = { candidateGatheredAt: result.hostCandidate.gatheredAt, startedAt, firstHitAt,
        firstHitAfterGatheredMs: firstHitAt - result.hostCandidate.gatheredAt,
        firstHitAfterStartedMs: firstHitAt - startedAt,
        firstApplicationRpcAt: result.appRpcPaths[0].at,
        stoppedAt: stopped?.at, durationMs: stopped?.at - startedAt,
        stopReason: stopped?.reason, addressesSent: stopped?.addresses_sent,
        addressesAttempted: stopped?.addresses_attempted,
        completedSweep: stopped?.reason === "completed" };
    }
    if (mode === "late-unresolved") {
      assert.equal(result.primed.restarts, 0, "the first late sweep pass alone keeps TURN");
      assert.equal(result.after.restarts, 1, "one optional restart restores native host checking");
      assert(wire.browser_checks.some((row) => row.at > indications[0].at && row.has_integrity),
        "fresh-generation authenticated checks follow the exhausted initial checklist");
      const restartAt = result.diagnostics.find((row) => row.event === "direct-pair" && row.state === "trying")?.at;
      assert(restartAt > result.exhaustedCheckWindow.observedAt * 1000,
        "the actual optional restart follows the exhausted original check window");
      const originalPort = result.before.direct[0].localPort;
      assert(!wire.browser_checks.some((row) => row.source_port === originalPort
        && row.at > result.exhaustedCheckWindow.observedAt && row.at < restartAt / 1000),
        "the exhausted original host port sends no new check until the one optional restart");
    }
  } else if (mode === "far-edge-pressure") {
    assert(wire.mdns_silenced && wire.browser_checks.length === 0,
      "the no-direct control keeps mDNS silent and drops native host checks before egress");
    assert(result.droppedHostChecks > 0, "the namespace firewall counts actual suppressed host packets");
    const stopped = sweepEvents.findLast((event) => event.reason === "window-expired");
    assert(stopped && stopped.addresses_sent > 0 && stopped.addresses_sent < 1021,
      "the mostly empty /22 expires with an honest incomplete first pass");
    assert.equal(stopped.prflx_followed, false, "suppressing inbound checks prevents direct learning");
    assert(wire.host_socket_indications.length > 0, "the actual far-edge phone still receives the credential-free probe");
    const started = sweepEvents.find((event) => event.status === "started");
    const pressure = JSON.parse(await readFile(path.join(artifacts, "pressure.json"), "utf8"));
    assert(pressure.length > 0, "the isolated namespace samples its actual advertised host socket");
    const peak = pressure.reduce((a, b) => a.tx_occupied / a.tx_capacity >= b.tx_occupied / b.tx_capacity ? a : b);
    result.measurements = { startedAt: started?.at, stoppedAt: stopped.at,
      durationMs: stopped.at - started?.at, reason: stopped.reason,
      addressesSent: stopped.addresses_sent, addressesAttempted: stopped.addresses_attempted,
      incompleteTail: true, droppedHostChecks: result.droppedHostChecks,
      pressureSamples: pressure.length, maxTxOccupied: peak.tx_occupied,
      txCapacityAtPeak: peak.tx_capacity, maxOccupiedFraction: peak.tx_occupied / peak.tx_capacity,
      minHeadroomAtPeak: peak.tx_capacity - peak.tx_occupied,
      neighborStatesAtPeak: peak.neighbor_states };
  } else {
    assert.equal(wire.host_socket_indications.length, 0, "a /21 must send no sweep datagrams");
    assert(sweepEvents.some((event) => event.reason === "subnet-too-large"), "the skip reason is observable");
    assert(sweepEvents.every((event) => event.addresses_sent === 0 && event.addresses_attempted === 0),
      "the oversized subnet sends and attempts zero sweep datagrams");
  }
  const completionDeadline = Date.now() + 3000;
  while (true) {
    try { await readFile(path.join(artifacts, "bridge-finished")); break; } catch (error) {
      if (error.code !== "ENOENT" || Date.now() >= completionDeadline) throw error;
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
  }
  const completedLog = await readFile(path.join(artifacts, "bridge.log"), "utf8");
  const sessionSummary = completedLog.split("\n").find((line) => line.includes("session lan-upgrade-fixture summary:"));
  assert(sessionSummary, "the same encrypted session emits a final transport summary");
  assert.match(sessionSummary, new RegExp(`\\b${result.after.restarts} ICE restarts?\\b`),
    "the summary counts actual changed-credential restarts");
  if (mode === "delayed") {
    assert.match(sessionSummary, /carried \d+s over turn and [1-9]\d*s over direct/,
      "the same session's final summary retains its real time on direct");
  } else if (startsDirect) {
    assert.match(sessionSummary, /carried \d+s over direct, 0 ICE restarts/,
      "the initially direct session has no TURN carrying history");
  }
  assert(wire.queries.length > 0 && wire.queries.every((query) => query.port === 5353 && query.class === 1),
    "bridge queries mDNS from5353 usingQM");
  await save("result.json", { ...result, finalChecks, sweepEvents, sessionSummary, wire });
  console.log("PASS", JSON.stringify({ before: result.before, viable: result.viable, after: result.after, wire }));
} catch (error) {
  const snapshot = await page?.evaluate(() => window.fixtureSnapshot?.()).catch(() => null);
  const dropped = mode === "far-edge-pressure" ? await droppedHostChecks().catch(() => null) : undefined;
  await save("failure.json", { error: String(error), snapshot, droppedHostChecks: dropped });
  console.error("fixture failure", JSON.stringify(snapshot));
  console.error(error);
  process.exitCode = 1;
} finally {
  await browser?.close();
  await server.close();
}
