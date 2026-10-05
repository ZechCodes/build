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
const unknown = mode.startsWith("unknown-neighbor-");
const pressureControl = ["far-edge-pressure", "unknown-neighbor-pressure"].includes(mode);
const natControl = ["different-nat", "missing-srflx"].includes(mode);
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
    const privateChecks = wire.browser_checks.filter((row) => row.source_port === sourcePort
      && row.destination_port === destinationPort && row.has_username && row.has_integrity);
    const srflxChecks = wire.browser_srflx_checks.filter((row) => row.source_port === sourcePort);
    const checks = [...privateChecks, ...srflxChecks].sort((left, right) => left.at - right.at);
    return { checks: checks.length, privateChecks: privateChecks.length, srflxChecks: srflxChecks.length,
      firstAt: checks[0]?.at, lastAt: checks.at(-1)?.at,
      observedAt: Date.now() / 1000 };
  };
  await page.exposeFunction("fixtureInitialCheckWindow", initialCheckWindow);
  await page.exposeFunction("fixtureInitialChecksExhausted", async (sourcePort, destinationPort) => {
    const window = await initialCheckWindow(sourcePort, destinationPort);
    // Actual srflx adds a second remote tuple sharing this original socket.
    // Use the measured total budget and age only as a release trigger;
    // retrospective private-tuple silence through the restart proves that
    // the later real indication alone did not revive that host pair.
    return window.privateChecks > 0 && window.checks >= 31 && window.observedAt - window.firstAt >= 15;
  });
  await page.exposeFunction("fixtureExhausted", (state) => save("exhausted.json", state));
  await page.exposeFunction("fixtureLatePrimed", (state) => save("late-primed.json", state));
  const gatheredHosts = [];
  const gatheredSrflx = [];
  let srflxSave = Promise.resolve();
  await page.exposeFunction("fixtureSrflx", async (candidate) => {
    gatheredSrflx.push(candidate);
    srflxSave = srflxSave.then(() => save("gathered-srflx.json", gatheredSrflx));
    await srflxSave;
  });
  await page.exposeFunction("fixtureBridgeHostPort", (port) => save("bridge-host-socket.json", { port }));
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
      try {
        await readFile(path.join(artifacts, "host-disconnected"));
        await readFile(path.join(artifacts, "service-host-disconnected"));
        return;
      } catch (error) {
        if (error.code !== "ENOENT" || Date.now() >= deadline) throw error;
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
    }
  });
  await page.exposeFunction("fixtureBridgeFailed", async () =>
    /ice_state=failed\b/.test(await readFile(path.join(artifacts, "bridge.log"), "utf8")));
  await page.exposeFunction("fixtureSweepSkipped", async () =>
    /subnet-too-large/.test(await readFile(path.join(artifacts, "bridge.log"), "utf8")));
  await page.exposeFunction("fixtureNatSkipped", async () =>
    /host-sweep .*"reason":"nat-(?:evidence-missing|address-mismatch)".*"status":"skipped"/.test(
      await readFile(path.join(artifacts, "bridge.log"), "utf8")));
  await page.exposeFunction("fixtureSweepPrimed", async () => {
    const wire = JSON.parse(await readFile(path.join(artifacts, "wire.json"), "utf8"));
    const { before } = JSON.parse(await readFile(path.join(artifacts, "before.json"), "utf8"));
    const host = JSON.parse(await readFile(path.join(artifacts, "host.json"), "utf8"));
    return wire.host_socket_indications.some((row) => row.source_port === before.hostSocketPort && row.destination_port === host.port);
  });
  await page.exposeFunction("fixtureSweepFinished", async () =>
    /host-sweep .*"reason":"(?:window-expired|completed)"/.test(await readFile(path.join(artifacts, "bridge.log"), "utf8")));
  await page.exposeFunction("fixtureDroppedHostChecks", droppedHostChecks);
  await page.goto(`http://localhost:9001/fixture${unknown ? "?topology=unknown" : ""}`);
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
  await srflxSave;
  const observedStun = JSON.parse(await readFile(path.join(artifacts, "stun-observed.json"), "utf8"));
  const ipv4Srflx = gatheredSrflx.filter((candidate) => candidate.protocol === "udp" && candidate.component === 1
    && /^\d+\.\d+\.\d+\.\d+$/.test(candidate.ip));
  const browserSrflx = ipv4Srflx.filter((candidate) => candidate.side === "browser");
  const bridgeSrflx = ipv4Srflx.filter((candidate) => candidate.side === "bridge");
  const commonIp = browserSrflx.some((left) => bridgeSrflx.some((right) => left.ip === right.ip));
  result.natEvidence = { browser: browserSrflx, bridge: bridgeSrflx, commonIpv4: commonIp, observedStun };
  assert(observedStun.length > 0, "the external namespace observes actual STUN requests through kernel SNAT");
  if (mode === "missing-srflx") {
    assert.equal(ipv4Srflx.length, 0, "withholding STUN yields no actual signaled IPv4 srflx candidate");
  } else {
    assert(browserSrflx.length > 0 && bridgeSrflx.length > 0, "both peers gather real IPv4 srflx through kernel SNAT");
    assert(ipv4Srflx.every((candidate) => observedStun.some((tuple) => tuple.ip === candidate.ip && tuple.port === candidate.port)),
      "every signaled srflx tuple was actually observed by the independent STUN service");
    assert.equal(commonIp, mode !== "different-nat", "the actual public candidate sets match only in equal-NAT cases");
  }
  if (mode === "delayed") {
    assert.equal(finalChecks?.reason, "direct-checks-succeeded", "actual ICE Failed must retain its successful host history at rtc.close");
    assert(finalChecks.hosts.some((host) => host.requests_sent > 0 && host.responses_received > 0 && host.succeeded),
      "the final host diagnostic retains real request/reply counters and success");
    assert(bridgeLog.indexOf("ice_state=failed") < bridgeLog.lastIndexOf("remote_candidates "),
      "the final host diagnostics follow actual bridge ICE Failed");
    assert(wire.host_checks.some((row) => row.after_release), "bridge sends outbound late-host STUN checks");
  } else if (["early-unresolved", "far-edge-unresolved", "unresolved", "late-unresolved", "unknown-neighbor-unresolved", "unknown-neighbor-clustered"].includes(mode)) {
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
  } else if (pressureControl) {
    assert(wire.mdns_silenced && wire.browser_checks.length === 0,
      "the no-direct control keeps mDNS silent and drops native host checks before egress");
    assert(result.droppedHostChecks > 0, "the namespace firewall counts actual suppressed host packets");
    const stopped = sweepEvents.findLast((event) => ["window-expired", "completed"].includes(event.reason));
    assert(stopped && stopped.addresses_sent > 0 && stopped.addresses_sent < 1021,
      "the mostly empty /22 terminates with actual real ICE send counts");
    assert.equal(stopped.prflx_followed, false, "suppressing inbound checks prevents direct learning");
    assert(wire.host_socket_indications.length > 0, "the actual far-edge phone still receives the credential-free probe");
    if (stopped.reason === "completed") {
      assert.equal(stopped.destinations_scouted, unknown ? 1020 : 1019,
        "completion covers all /22 destinations except genuinely known gateway and phone neighbors");
      assert.equal(stopped.neighbors_pending, 0, "completion settles admitted ARP attempts");
      assert.equal(wire.host_socket_indications.length, 2,
        "completion includes the initial and bounded repeat from the real host socket to the phone");
    }
    const started = sweepEvents.find((event) => event.status === "started");
    const pressure = JSON.parse(await readFile(path.join(artifacts, "pressure.json"), "utf8"));
    assert(pressure.length > 0, "the isolated namespace samples its actual advertised host socket");
    const peak = pressure.reduce((a, b) => a.tx_occupied / a.tx_capacity >= b.tx_occupied / b.tx_capacity ? a : b);
    result.measurements = { startedAt: started?.at, stoppedAt: stopped.at,
      durationMs: stopped.at - started?.at, reason: stopped.reason,
      addressesSent: stopped.addresses_sent, addressesAttempted: stopped.addresses_attempted,
      incompleteTail: stopped.reason === "window-expired", droppedHostChecks: result.droppedHostChecks,
      pressureSamples: pressure.length, maxTxOccupied: peak.tx_occupied,
      txCapacityAtPeak: peak.tx_capacity, maxOccupiedFraction: peak.tx_occupied / peak.tx_capacity,
      minHeadroomAtPeak: peak.tx_capacity - peak.tx_occupied,
      neighborStatesAtPeak: peak.neighbor_states };
    result.measurements.arp = arpPressure(wire, pressure);
  } else if (natControl) {
    const reason = mode === "different-nat" ? "nat-address-mismatch" : "nat-evidence-missing";
    assert.equal(wire.host_socket_indications.length, 0, "ineligible NAT evidence sends no real host-socket indication");
    assert.equal(wire.scout_hits.length, 0, "ineligible NAT evidence sends no anonymous scout to the phone");
    assert(sweepEvents.some((event) => event.reason === reason && event.status === "skipped"), "the fixed NAT skip reason is observable");
    assert(sweepEvents.every((event) => event.addresses_sent === 0 && event.addresses_attempted === 0
      && event.scout_datagrams_sent === 0 && event.scout_attempted === 0 && !event.eligible),
      "missing or disjoint public evidence suppresses both kinds of feature traffic for the complete lifetime");
  } else {
    assert.equal(wire.host_socket_indications.length, 0, "a /21 must send no sweep datagrams");
    assert(sweepEvents.some((event) => event.reason === "subnet-too-large"), "the skip reason is observable");
    assert(sweepEvents.every((event) => event.addresses_sent === 0 && event.addresses_attempted === 0),
      "the oversized subnet sends and attempts zero sweep datagrams");
  }
  if (unknown) {
    const neighborBefore = JSON.parse(await readFile(path.join(artifacts, "neighbor-before.json"), "utf8"));
    assert(neighborBefore.phone_absent && neighborBefore.proxy_entries === 0,
      "the actual far-edge phone has no neighbor or proxy entry before authentic trickle");
    const requests = wire.phone_arp_requests.filter((row) => row.at >= neighborBefore.at);
    const replies = wire.phone_arp_replies.filter((row) => row.at >= neighborBefore.at);
    assert(requests.length > 0 && replies.length > 0 && replies[0].at >= requests[0].at,
      "real bridge ARP discovers the previously absent far-edge phone");
    const indication = wire.host_socket_indications[0];
    assert(indication && indication.at >= replies[0].at,
      "the actual host-socket indication follows real ARP discovery");
    assert(wire.scout_hits.some((row) => row.source_port !== result.before.hostSocketPort
      && row.destination_port === 9 && row.bytes === 1 && row.at <= indication.at),
      "the discovery datagram uses a separate ephemeral socket and only one anonymous byte to UDP9");
    if (mode === "unknown-neighbor-clustered") {
      assert.equal(result.after.restarts, 0, "unknown neighbor is discovered during the original browser retry window");
      assert.equal(result.before.localUfrag, result.after.localUfrag);
      assert.equal(result.before.remoteUfrag, result.after.remoteUfrag);
      assert(result.appRpcPaths.every((row) => row.path === "direct"),
        "clustered unknown discovery carries every application RPC on direct");
      assert(bridgeLog.split("\n").filter((line) => line.includes("carrying over "))
        .every((line) => line.includes("host/prflx candidates")),
      "the clustered unknown bridge never carries application data on TURN");
    } else if (mode === "unknown-neighbor-unresolved") {
      assert.equal(result.after.restarts, 1, "the far-edge discovery uses exactly the existing one optional restart");
      assert.notEqual(result.before.localUfrag, result.after.localUfrag);
      assert.notEqual(result.before.remoteUfrag, result.after.remoteUfrag);
      assert((indication.at - neighborBefore.at) * 1000 < 25000,
        "far-edge discovery reaches the real phone inside the unchanged 25-second lifetime");
      assert.equal(result.appRpcPaths[0].path, "turn", "the same encrypted session starts on genuine TURN");
      assert.equal(result.appRpcPaths.at(-1).path, "direct", "the same encrypted session finishes on direct");
      const restarted = sweepEvents.filter((event) => event.generation === 2);
      assert(restarted.length > 0 && restarted.every((event) => event.scout_datagrams_sent === 0 && event.scout_attempted === 0),
        "the generation inside the 60-second interface lease emits zero anonymous scouts");
      assert(restarted.some((event) => event.reason === "interface-scout-cooldown"),
        "the interface scout cooldown is observable on the existing optional restart");
      assert(restarted.some((event) => event.addresses_sent > 0),
        "fresh usable neighbors still receive real host-socket indications during scout cooldown");
    }
    const gathered = JSON.parse(await readFile(path.join(artifacts, "host.json"), "utf8"));
    const startedAt = sweepEvents.find((event) => event.generation === 1 && event.status === "started")?.at;
    result.unknownNeighbor = { neighborBefore, candidateGatheredAt: gathered.gatheredAt, startedAt,
      firstPhoneArpRequestAt: requests[0].at,
      firstPhoneArpReplyAt: replies[0].at, firstHostIndicationAt: indication.at,
      firstHitAfterAbsentMs: (indication.at - neighborBefore.at) * 1000,
      firstHitAfterGatheredMs: indication.at * 1000 - gathered.gatheredAt,
      firstHitAfterStartedMs: indication.at * 1000 - startedAt,
      firstViableObservedAt: result.initialViable?.at,
      directObservedAt: result.directObservedAt, restarts: result.after.restarts,
      directObservedAfterFirstHitMs: result.directObservedAt ? result.directObservedAt - indication.at * 1000 : undefined,
      initialApplicationPath: result.appRpcPaths[0]?.path,
      finalApplicationPath: result.appRpcPaths.at(-1)?.path,
      arpRequests: wire.arp_requests, uniqueArpDestinations: wire.arp_unique_destinations };
    const generationEvents = sweepEvents.filter((event) => event.generation === 1);
    const terminal = sweepEvents.findLast((event) => event.status === "stopped");
    const latest = generationEvents.at(-1);
    const admitted = generationEvents.find((event) => event.destinations_scouted === 1020);
    result.unknownNeighbor.discovery = { scoutDatagramsSent: latest?.scout_datagrams_sent,
      scoutAttempts: latest?.scout_attempted, destinationsScouted: latest?.destinations_scouted,
      neighborsPending: latest?.neighbors_pending, neighborsPendingPeak: latest?.neighbors_pending_peak,
      realHostDatagramsSent: latest?.addresses_sent, realHostAttempts: latest?.addresses_attempted,
      fullScoutAdmissionObservedAt: admitted?.at, finalNewArpRequestAt: wire.last_new_arp_request_at,
      fullArpCoverageAt: wire.arp_unique_destinations >= 1020 ? wire.last_new_arp_request_at : undefined,
      stopAt: terminal?.at, stopReason: terminal?.reason,
      stopGeneration: terminal?.generation,
      fullScoutAdmissionObserved: latest?.destinations_scouted === 1020,
      completedDiscoveryPass: terminal?.reason === "completed" };
    if (pressureControl) {
      // One genuine gateway is already usable; all other1020 eligible /22
      // destinations must be admitted by the anonymous scouts and actually
      // receive ARP. Enqueue coverage is distinct from neighbor resolution.
      assert.equal(latest?.destinations_scouted, 1020, "the complete unknown /22 tail is actually scouted");
      assert(wire.arp_unique_destinations >= 1020, "packet capture proves actual full ARP coverage");
      assert(latest?.neighbors_pending_peak <= 256, "discovery retains its global bounded neighbor pressure");
      if (terminal?.reason === "completed") {
        assert.equal(terminal.neighbors_pending, 0, "completion settles admitted ARP attempts");
      }
    } else {
      assert.equal(terminal?.reason, "direct-selected", "the actual direct selection cancels discovery");
      assert.equal(result.unknownNeighbor.discovery.completedDiscoveryPass, false,
        "direct cancellation must not be labeled a completed discovery pass");
    }
    const pressure = JSON.parse(await readFile(path.join(artifacts, "pressure.json"), "utf8"));
    assert(pressure.length > 0, "the actual ICE host socket is sampled during unknown-neighbor discovery");
    const peak = pressure.reduce((a, b) => a.tx_occupied / a.tx_capacity >= b.tx_occupied / b.tx_capacity ? a : b);
    result.unknownNeighbor.pressure = { samples: pressure.length, maxTxOccupied: peak.tx_occupied,
      capacityAtPeak: peak.tx_capacity, maxOccupiedFraction: peak.tx_occupied / peak.tx_capacity,
      maxIncompleteNeighbors: Math.max(...pressure.map((sample) => sample.neighbor_states.INCOMPLETE || 0)) };
    result.unknownNeighbor.pressure.arp = arpPressure(wire, pressure);
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
  } else if (startsDirect || mode === "unknown-neighbor-clustered") {
    assert.match(sessionSummary, /carried \d+s over direct, 0 ICE restarts/,
      "the initially direct session has no TURN carrying history");
  }
  assert(wire.queries.length > 0 && wire.queries.every((query) => query.port === 5353 && query.class === 1),
    "bridge queries mDNS from5353 usingQM");
  await save("result.json", { ...result, finalChecks, sweepEvents, sessionSummary, wire });
  console.log("PASS", JSON.stringify({ before: result.before, viable: result.viable, after: result.after, wire }));
} catch (error) {
  const snapshot = await page?.evaluate(() => window.fixtureSnapshot?.()).catch(() => null);
  const dropped = pressureControl ? await droppedHostChecks().catch(() => null) : undefined;
  await save("failure.json", { error: String(error), snapshot, droppedHostChecks: dropped });
  console.error("fixture failure", JSON.stringify(snapshot));
  console.error(error);
  process.exitCode = 1;
} finally {
  await browser?.close();
  await server.close();
}

function arpPressure(wire, samples) {
  const maxGlobal = samples.reduce((a, b) => a.global_arp_entries >= b.global_arp_entries ? a : b);
  const fulls = samples.map((sample) => sample.global_table_fulls);
  const elapsed = wire.last_arp_request_at - wire.first_arp_request_at;
  return { maxGlobalEntries: maxGlobal.global_arp_entries, gcThresh2: maxGlobal.global_gc_thresh2,
    gcThresh3: maxGlobal.global_gc_thresh3,
    globalTableFullsDelta: Math.max(...fulls) - Math.min(...fulls),
    requestsIncludingKernelRetries: wire.arp_requests,
    maxOneSecondBinPps: Math.max(...Object.values(wire.arp_one_second_bins)),
    averagePps: wire.arp_requests / elapsed, observedDurationSeconds: elapsed };
}
