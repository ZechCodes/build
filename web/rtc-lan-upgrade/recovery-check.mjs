import assert from "node:assert/strict";

export function assertRecovery(result, sweepEvents, wire, bridgeLog) {
  assert.equal(result.after.restarts, 2, "one recovery and one optional upgrade run natively");
  assert.equal(result.after.connections, 1, "the encrypted session keeps its RTCPeerConnection");
  assert.equal(result.turnPull.sessionId, result.directPull.sessionId);
  const attempts = result.diagnostics.filter((row) => row.event === "direct-pair" && row.state === "trying");
  assert.equal(attempts.length, 1, "the existing optional upgrade budget is spent once");
  assert(attempts[0].at >= result.reconnectAt + 19000, "the production 20-second settling policy remains intact");
  assert.equal(attempts[0].reason, "conntrack-sweep", "unresolved host sweep warrants fresh nomination");
  assert(wire.mdns_silenced, "all native generations retain unresolved real Chromium mDNS");
  const started = sweepEvents.filter((row) => row.status === "started");
  assert(started.length >= 3, "initial, recovery and optional generations start actual native sweeps");
  for (const event of started) {
    assert(event.addresses_sent <= 1 && event.addresses_attempted <= 1,
      "the first native event contains only a possible new-generation early probe");
    assert.equal(event.prflx_followed, false, "fresh native sweep has not followed an old PRFLX");
  }
  assert(sweepEvents.some((event) => event.generation === 2 && event.prflx_followed
    && event.eligible_unresolved === 1 && event.at < attempts[0].at),
  "recovery already followed authenticated PRFLX while unresolved evidence still warrants the optional restart");
  const generations = result.diagnostics.filter((row) => row.phase === "generation");
  assert.deepEqual(generations.map((row) => row.generation), [1, 2, 3]);
  assert(generations.every((row) => Object.values(row.candidates).every((count) => count === 0)),
    "every native credential boundary clears all candidate counters");
  assert.deepEqual(result.appRpcPaths.map((row) => row.path),
    ["direct", "direct", "direct", "direct", "direct", "turn", "turn", "turn", "turn", "direct", "direct", "direct", "direct"],
  "the complete encrypted pulls carry on their measured initial, recovered and upgraded paths");
  assert.equal(result.reconnectToDirectMs, result.carryingDirectAt - result.reconnectAt,
    "reconnect latency uses production diagnostic timestamps");
  const freshProbe = wire.host_socket_indications.find((row) => row.at * 1000 >= attempts[0].at
    && row.source_port === result.after.selected.remotePort && row.destination_port === result.after.selected.localPort);
  assert(freshProbe, "the optional native generation probes the actual selected direct tuple");
  assert.equal(freshProbe.bytes, 28);
  assert(freshProbe.has_fingerprint && !freshProbe.has_username && !freshProbe.has_integrity,
    "the real host-socket probe contains no credentials");
  assert(wire.browser_checks.some((row) => row.at >= freshProbe.at
    && row.source_port === freshProbe.destination_port && row.destination_port === freshProbe.source_port
    && row.has_username && row.has_integrity), "authenticated native checks follow the fresh direct probe");
  assert.match(bridgeLog, /carrying over host\/prflx candidates/);
  assert(result.appRpcPaths.some((row) => row.path === "turn") && result.appRpcPaths.at(-1).path === "direct",
    "encrypted application RPCs carry first direct, recovered TURN and upgraded direct");
}
