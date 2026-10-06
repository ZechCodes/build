import assert from "node:assert/strict";
import { readFile, rename, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { createServer } from "../../spa/node_modules/vite/dist/node/index.js";
import { chromium } from "../../spa/node_modules/playwright-core/index.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const artifacts = process.argv[2];
const mode = process.env.BUILD_RTC_LAN_MODE;
const save = async (name, value) => {
  const target = path.join(artifacts, name);
  await writeFile(`${target}.tmp`, typeof value === "string" ? value : JSON.stringify(value, null, 2));
  await rename(`${target}.tmp`, target);
};
const server = await createServer({
  root: path.resolve(here, "../../spa"), configFile: false,
  server: { host: "127.0.0.1", port: 9001, strictPort: true, hmr: false, watch: null,
    fs: { allow: [path.resolve(here, "../../..")] } },
  plugins: [{ name: "arp-observation", resolveId(id) {
    if (id === "/__lan_fixture__.mjs") return path.join(here, "browser.mjs");
    if (id === "/__arp_observation__.mjs") return path.join(here, "arp-browser.mjs");
  }, configureServer(vite) {
    vite.middlewares.use("/fixture", (_request, response) => {
      response.setHeader("Content-Type", "text/html");
      response.end('<script type="module" src="/__lan_fixture__.mjs"></script>');
    });
  } }],
});
let browser;
let page;
try {
  await server.listen();
  browser = await chromium.launch({ executablePath: "/usr/bin/chromium", headless: true,
    args: ["--no-sandbox", "--disable-dev-shm-usage"] });
  page = await browser.newPage();
  page.on("console", (event) => console.log(event.type(), event.text()));
  page.on("pageerror", (error) => console.error("page error", error));
  const answers = [];
  const hosts = [];
  const srflx = [];
  await page.exposeFunction("fixtureMode", () => mode);
  await page.exposeFunction("fixtureReady", () => save("browser-ready", "ready"));
  await page.exposeFunction("fixtureAnswer", async (answer) => { answers.push(answer); await save("answers.json", answers); });
  await page.exposeFunction("fixtureBridgeHostPort", (port) => save("bridge-host-socket.json", { port }));
  await page.exposeFunction("fixtureHostPort", async (host) => { hosts.push(host); await save("gathered-hosts.json", hosts); });
  await page.exposeFunction("fixtureSrflx", async (candidate) => { srflx.push(candidate); await save("gathered-srflx.json", srflx); });
  await page.exposeFunction("fixtureHost", async (host) => {
    await save("host.json", host);
    await save("gate", "ready");
    const deadline = Date.now() + 3000;
    while (true) {
      try { await readFile(path.join(artifacts, "gated")); break; } catch (error) {
        if (error.code !== "ENOENT" || Date.now() >= deadline) throw error;
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
    }
  });
  await page.goto("http://127.0.0.1:9001/fixture?topology=unknown");
  await page.waitForFunction(() => typeof window.runLanUpgrade === "function");
  const firewallDeadline = Date.now() + 3000;
  while (true) {
    try { await readFile(path.join(artifacts, "firewall-ready")); break; } catch (error) {
      if (error.code !== "ENOENT" || Date.now() >= firewallDeadline) throw error;
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
  }
  const result = await page.evaluate(() => window.runLanUpgrade());
  await save("result.json", { ...result, answers, hosts, srflx });
  assert.equal(result.after.connections, 1, "observation keeps the production peer");
  assert(result.after.restarts <= 1, "the production restart budget is unchanged");
  assert.equal(result.firstPull.projectCount, 1);
  assert.equal(result.finalPull.projectCount, 1);
  assert(srflx.some((candidate) => candidate.side === "browser") && srflx.some((candidate) => candidate.side === "bridge"));
  const selected = result.after.selected;
  assert(selected?.state === "succeeded" && selected.nominated, "the observation finishes with a carrying pair");
  const selectedDirect = selected.localType !== "relay" && selected.remoteType !== "relay";
  assert(selectedDirect, "production discovery selects a native direct pair");
  if (mode === "arp-cold") assert.equal(result.after.restarts, 0, "cold on-link upgrades without an ICE restart");
  result.packetEvidence = await verifyPacketEvidence(mode, artifacts);
  await save("result.json", { ...result, answers, hosts, srflx });
  console.log("PASS", JSON.stringify({ mode, before: result.before.selected, after: result.after.selected, restarts: result.after.restarts }));
} catch (error) {
  const snapshot = await page?.evaluate(() => window.fixtureSnapshot?.()).catch(() => null);
  await save("failure.json", { error: String(error), snapshot });
  console.error(error);
  process.exitCode = 1;
} finally {
  await browser?.close();
  await server.close();
}

async function verifyPacketEvidence(mode, artifacts) {
  const log = await readFile(path.join(artifacts, "bridge.log"), "utf8");
  const sweep = log.split("\n").filter((line) => line.includes("host-sweep "))
    .map((line) => JSON.parse(line.split("host-sweep ")[1]));
  const lines = await readFile(path.join(artifacts, "bridge-observe.jsonl"), "utf8");
  const events = lines.trim().split("\n").filter(Boolean).map((line) => JSON.parse(line));
  const arp = events.filter((event) => event.kind === "arp" && event.operation === "request" && event.direction === "inbound");
  const indications = events.filter((event) => event.stun_kind === "binding_indication");
  const wire = JSON.parse(await readFile(path.join(artifacts, "wire.json"), "utf8"));
  if (mode === "arp-suppressed") {
    assert.equal(events.filter((event) => event.kind === "scout").length, 0, "no scout crosses the bridge interface");
    assert(sweep.length > 0 && sweep.every((event) => event.scout_datagrams_sent === 0), "the kernel rejects every scout enqueue");
  }
  if (mode === "arp-cold") {
    assert(sweep.length > 0 && sweep.every((event) => event.scout_datagrams_sent === 0),
      "cold on-link production path enqueues zero anonymous scouts");
    assert.equal(wire.arp_requests, 0, "cold on-link production path causes zero bridge ARP requests");
    assert.equal(wire.arp_unique_destinations, 0, "cold on-link production path opens no anonymous ARP destinations");
  }
  if (["arp-cold", "arp-suppressed", "arp-stale"].includes(mode)) {
    assert(arp.length > 0, "the phone supplies its own ARP");
    assert(indications.length > 0 && indications[0].at > arp[0].at, "the real probe follows ARP learning");
    const bridge = JSON.parse(await readFile(path.join(artifacts, "bridge-host-socket.json"), "utf8"));
    assert.equal(indications[0].source_port, bridge.port, "the probe leaves the advertised socket");
  }
  const authenticatedRequests = events.filter((event) => event.kind === "stun" && event.direction === "inbound"
    && event.request_with_credentials);
  const authenticatedReplies = events.filter((event) => event.kind === "stun" && event.direction === "outbound"
    && event.stun_kind === "binding_success");
  const exchange = authenticatedRequests.find((request) => authenticatedReplies.some((reply) =>
    reply.transaction === request.transaction && reply.at >= request.at));
  assert(exchange,
  "the direct pair exchanges a matching authenticated STUN check and success");
  assert.match(log, /carrying over host\/prflx candidates/, "the bridge authenticates the direct peer");
  if (mode === "arp-stale") assert(arp.every((event) => event.destination_mac !== "ff:ff:ff:ff:ff:ff"), "cached stale MACs refresh through unicast ARP");
  if (["arp-cached", "arp-never-arps"].includes(mode)) {
    assert.equal(arp.length, 0, "the asymmetric control never supplies ARP to the bridge");
    assert(sweep.some((event) => event.scout_datagrams_sent > 0),
      "the one-sided cache falls back to production anonymous scouting");
    assert(wire.arp_unique_destinations > 0, "the fallback emits real bridge ARP on the isolated LAN");
    assert(indications.length > 0, "scout discovery makes real host-socket probes possible");
  }
  return { firstPhoneArpAt: arp[0]?.at, firstHostIndicationAt: indications[0]?.at,
    firstAuthenticatedExchangeAt: exchange.at,
    scoutDatagramsSent: Math.max(...sweep.map((event) => event.scout_datagrams_sent)),
    bridgeArpRequests: wire.arp_requests, uniqueBridgeArpDestinations: wire.arp_unique_destinations };
}
