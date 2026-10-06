// The upgrade, whole job in one call: ICE servers, the two negotiated channels,
// the offer and the trickle both ways, settled once both channels are open.
// A failure anywhere leaves the caller on the relay, with no retry loop.

import { afterEach, beforeEach, describe, it, expect, vi } from "vitest";
import { openPeerLink } from "../src/core/peerLink.js";
import { clearConnectionDiagnosticHistory, connectionDiagnosticHistory, connectionDiagnosticReport } from "../src/core/connectionDiagnostics.js";
import { diagnosticsJson } from "../src/core/connectionDiagnosticsModel.js";

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

class FakeEventTarget {
  constructor() {
    this.listeners = {};
  }
  addEventListener(type, fn) {
    (this.listeners[type] ||= []).push(fn);
  }
  removeEventListener(type, fn) {
    this.listeners[type] = (this.listeners[type] || []).filter((listener) => listener !== fn);
  }
  listenerCount(type) {
    return (this.listeners[type] || []).length;
  }
  emit(type, event = {}) {
    (this.listeners[type] || []).forEach((fn) => fn(event));
  }
}

class FakeChannel extends FakeEventTarget {
  constructor(label, init) {
    super();
    this.label = label;
    this.init = init;
    this.readyState = "connecting";
    this.sent = [];
    this.bufferedAmount = 0;
  }
  open() {
    this.readyState = "open";
    this.emit("open");
  }
  send(text) {
    this.sent.push(text);
  }
  close() {
    this.readyState = "closed";
    this.emit("close");
  }
}

class FakePeerConnection extends FakeEventTarget {
  constructor(config) {
    super();
    this.config = config;
    this.channels = new Map();
    this.localDescriptions = [];
    this.remoteDescriptions = [];
    this.remoteCandidates = [];
    this.connectionState = "new";
    this.iceGatheringState = "complete";
    this.closed = false;
    this.autoConnect = true;
    this.localCandidateType = "host";
    this.remoteCandidateType = "host";
    FakePeerConnection.instances.push(this);
  }
  createDataChannel(label, init) {
    const channel = new FakeChannel(label, init);
    this.channels.set(label, channel);
    return channel;
  }
  /** The stats a browser answers with once a pair has won. A case can say
   *  which pair that is; the default is a direct one. */
  async getStats() {
    return new Map(
      [
        { id: "pair-1", type: "candidate-pair", state: "succeeded", nominated: true, localCandidateId: "l", remoteCandidateId: "r" },
        { id: "l", type: "local-candidate", candidateType: this.localCandidateType },
        { id: "r", type: "remote-candidate", candidateType: this.remoteCandidateType },
      ].map((entry) => [entry.id, entry]),
    );
  }
  async createOffer(options = {}) {
    return { type: "offer", sdp: `v=0 offer ${this.localDescriptions.length}${options.iceRestart ? " restart" : ""}` };
  }
  async setLocalDescription(description) {
    this.localDescriptions.push(description);
  }
  async setRemoteDescription(description) {
    this.remoteDescriptions.push(description);
    if (this.autoConnect && [...this.channels.values()].every(({ readyState }) => readyState === "open")) {
      this.connectionState = "connected";
      this.emit("connectionstatechange");
    }
  }
  setConfiguration(config) {
    this.config = config;
  }
  async addIceCandidate(candidate) {
    this.remoteCandidates.push(candidate);
  }
  close() {
    this.closed = true;
    this.connectionState = "closed";
    for (const channel of this.channels.values()) if (channel.readyState !== "closed") channel.close();
  }
  fail() {
    this.connectionState = "failed";
    this.emit("connectionstatechange");
  }
  gather(candidate) {
    this.emit("icecandidate", { candidate });
  }
}
FakePeerConnection.instances = [];

const SERVERS = [{ urls: ["stun:stun.cloudflare.com:3478"] }];

function stand({ signalImpl, servers = SERVERS, ...rest } = {}) {
  FakePeerConnection.instances.length = 0;
  const signalled = [];
  const candidateSinks = [];
  const respond =
    signalImpl || (async (method) => (method === "rtc.offer" ? { sdp: "v=0 answer" } : {}));
  const signal = (method, params) => {
    signalled.push([method, params]);
    return respond(method, params);
  };
  const fetchIceServers = vi.fn(async () => servers);
  const link = openPeerLink({
    signal,
    fetchIceServers,
    RTCPeerConnectionImpl: FakePeerConnection,
    onPush: (deliver) => {
      candidateSinks.push(deliver);
      return () => candidateSinks.splice(candidateSinks.indexOf(deliver), 1);
    },
    ...rest,
  });
  link.catch(() => {});
  return { link, signalled, fetchIceServers, candidateSinks, peer: () => FakePeerConnection.instances.at(-1) };
}

async function upgrade(options) {
  const stood = stand(options);
  await tick();
  const peer = stood.peer();
  peer.channels.get("app").open();
  peer.channels.get("term").open();
  peer.connectionState = "connected";
  peer.emit("connectionstatechange");
  return { ...stood, peer, resolved: await stood.link };
}

/** A pair in a stats report, and the two candidates it is made of. */
const pairEntries = (id, { localType, remoteType, state = "succeeded", nominated = false }) => [
  { id, type: "candidate-pair", state, nominated, localCandidateId: `${id}-l`, remoteCandidateId: `${id}-r` },
  { id: `${id}-l`, type: "local-candidate", candidateType: localType },
  { id: `${id}-r`, type: "remote-candidate", candidateType: remoteType },
];

/** A session that landed on TURN, over a check list the case chooses. The
 *  interesting one is the maintainer's: a direct pair that ALSO succeeded,
 *  which the relay pair merely beat to nomination. */
const asReport = (entries) => new Map(entries.map((entry) => [entry.id, entry]));

async function landed({ report, path, ...options }) {
  const stood = stand(options);
  await vi.advanceTimersByTimeAsync(0);
  const peer = stood.peer();
  peer.getStats = report;
  peer.channels.get("app").open();
  peer.channels.get("term").open();
  peer.connectionState = "connected";
  peer.emit("connectionstatechange");
  await vi.advanceTimersByTimeAsync(0);
  const resolved = await stood.link;
  expect(resolved.transportPath()).toBe(path);
  return { ...stood, peer, resolved };
}

/** A session that landed on TURN, over a check list the case chooses. The
 *  interesting one is the maintainer's: a direct pair that ALSO succeeded,
 *  which the relay pair merely beat to nomination. */
const landedOnRelay = ({ alsoDirect = "succeeded", ...options } = {}) =>
  landed({
    ...options,
    path: "turn",
    report: async () =>
      asReport([
        ...pairEntries("relayed", { localType: "relay", remoteType: "host", nominated: true }),
        ...(alsoDirect ? pairEntries("direct", { localType: "host", remoteType: "host", state: alsoDirect }) : []),
      ]),
  });

const landedDirect = () =>
  landed({
    path: "direct",
    report: async () => asReport(pairEntries("direct", { localType: "host", remoteType: "host", nominated: true })),
  });

const offers = (signalled) => signalled.filter(([method]) => method === "rtc.offer").length;
const hostSweepDiagnostic = (generation, { candidateReason = "mdns-pending", eligible = true, status = "started", candidates = { mdns_pending: 1 },
  eligible_unresolved = eligible && status !== "skipped" ? 1 : 0, ...sweep } = {}) => ({
  type: "rtc.diagnostics", event: "host-sweep", reason: candidateReason, candidates,
  sweep: { generation, status, eligible, eligible_unresolved, addresses_sent: 0, reason: null, prflx_followed: false, ...sweep },
});
const bridgeGenerationDiagnostic = (generation) => ({ type: "rtc.diagnostics", event: "generation", generation, reason: "no-host-candidates", candidates: {} });

/** What the connection recorded under one event, in order — point 3's half of
 *  #31: the diagnostics say which pair type a session landed on and whether it
 *  was ever re-nominated. */
const diagnosticsOf = (event) =>
  connectionDiagnosticHistory().filter((entry) => entry.event === event).map((entry) => entry.state);

describe("openPeerLink", () => {
  it("omits the optional client hint before its capability is known", async () => {
    const { resolved, signalled } = await upgrade({ clientId: () => null });
    expect(signalled.find(([method]) => method === "rtc.offer")[1]).not.toHaveProperty("client_id");
    resolved.close();
  });

  it("reads the client hint again for a restart after the greeting establishes support", async () => {
    let hint = null;
    const { resolved, peer, signalled } = await upgrade({ clientId: () => hint, confirmCarried: async () => true });
    const clientId = "773d4f16-a915-43cb-a067-c31bf8dfe1aa";
    hint = clientId;
    peer.fail();
    for (let index = 0; index < 6; index += 1) await tick();
    const offers = signalled.filter(([method]) => method === "rtc.offer");
    expect(offers).toHaveLength(2);
    expect(offers[0][1]).not.toHaveProperty("client_id");
    expect(offers[1][1]).toHaveProperty("client_id", clientId);
    expect(JSON.stringify(connectionDiagnosticHistory())).not.toContain(clientId);
    resolved.close();
  });
  it("offers two negotiated channels with the ICE servers it fetched, and settles when both open", async () => {
    const { peer, signalled, resolved, fetchIceServers } = await upgrade();
    expect(fetchIceServers).toHaveBeenCalledTimes(1);
    expect(peer.config.iceServers).toEqual(SERVERS);
    expect(peer.channels.get("app").init).toMatchObject({ negotiated: true, id: 0, ordered: true });
    expect(peer.channels.get("term").init).toMatchObject({ negotiated: true, id: 1, ordered: true });
    expect(signalled[0]).toEqual(["rtc.offer", { sdp: "v=0 offer 0", ice_servers: SERVERS }]);
    expect(peer.remoteDescriptions).toEqual([{ type: "answer", sdp: "v=0 answer" }]);
    expect(resolved.app.send).toBeTypeOf("function");
    expect(resolved.term.send).toBeTypeOf("function");
  });

  it("trickles its own candidates over the relay and feeds the bridge's to the peer", async () => {
    const { peer, signalled, candidateSinks } = await upgrade();
    peer.gather({ candidate: "candidate:1 1 udp", toJSON: () => ({ candidate: "candidate:1 1 udp" }) });
    peer.gather(null); // end of gathering says nothing
    await tick();
    expect(signalled.filter(([method]) => method === "rtc.ice")).toEqual([
      ["rtc.ice", { candidate: { candidate: "candidate:1 1 udp" } }],
    ]);
    // The bridge's push, verbatim: it names the one BUNDLE section its
    // candidate belongs to (bridge/src/rtc.rs, `placed_in_bundle`), so what
    // reaches `addIceCandidate` is what arrived — the SPA normalises nothing.
    const fromTheBridge = { candidate: "candidate:2 1 udp", sdpMid: "0", sdpMLineIndex: 0 };
    candidateSinks[0]({ type: "rtc.ice", candidate: fromTheBridge });
    await tick();
    expect(peer.remoteCandidates).toEqual([fromTheBridge]);
  });

  it("finishes initial signaling after late host gathering and its candidate reply without delaying the ready link", async () => {
    let answerCandidate;
    const stood = stand({ signalImpl: async (method) => {
      if (method === "rtc.offer") {
        FakePeerConnection.instances.at(-1).iceGatheringState = "gathering";
        return { sdp: "v=0 answer" };
      }
      if (method === "rtc.ice") return new Promise((resolve) => { answerCandidate = resolve; });
      return {};
    } });
    await tick();
    const peer = stood.peer();
    peer.iceGatheringState = "gathering";
    peer.channels.get("app").open();
    peer.channels.get("term").open();
    peer.connectionState = "connected";
    peer.emit("connectionstatechange");
    const resolved = await stood.link;
    try {
      expect(peer.iceGatheringState).toBe("gathering");
      expect(resolved.whenInitialSignalingComplete).toBeTypeOf("function");
      let finished = false;
      const completion = resolved.whenInitialSignalingComplete().then(() => { finished = true; });
      const candidate = {
        candidate: "candidate:1 1 udp 1 12345678-1234-4234-8234-123456789abc.local 5000 typ host",
        sdpMid: "0", sdpMLineIndex: 0,
      };
      peer.gather({ type: "host", ...candidate, toJSON: () => candidate });
      await tick();
      expect(stood.signalled.filter(([method]) => method === "rtc.ice")).toEqual([["rtc.ice", { candidate }]]);
      peer.iceGatheringState = "complete";
      peer.gather(null);
      peer.emit("icegatheringstatechange");
      await tick();
      expect(finished).toBe(false);

      answerCandidate({});
      await completion;
      expect(finished).toBe(true);
    } finally {
      resolved.close();
    }
  });

  it("keeps initial signaling through held relay candidate replies when gathering ends before the channels open", async () => {
    let answerCandidate;
    const stood = stand({ signalImpl: async (method, params) => {
      if (method === "rtc.offer") {
        FakePeerConnection.instances.at(-1).iceGatheringState = "gathering";
        return { sdp: "v=0 answer" };
      }
      if (method === "rtc.ice" && params.candidate.type === "relay") return new Promise((resolve) => { answerCandidate = resolve; });
      return {};
    } });
    await tick();
    const peer = stood.peer();
    peer.gather({ type: "host", candidate: "candidate:0 1 udp 1 host.local 5000 typ host" });
    await tick();
    peer.gather({ type: "relay", candidate: "candidate:1 1 udp 1 203.0.113.1 5000 typ relay" });
    peer.iceGatheringState = "complete";
    peer.emit("icegatheringstatechange");
    peer.gather(null);
    await tick();
    peer.channels.get("app").open();
    peer.channels.get("term").open();
    peer.connectionState = "connected";
    peer.emit("connectionstatechange");
    const link = await stood.link;
    try {
      let settled = false;
      const complete = link.whenInitialSignalingComplete();
      void complete.then(() => { settled = true; });
      await tick();
      expect(answerCandidate).toBeTypeOf("function");
      expect(settled).toBe(false);
      answerCandidate({});
      await complete;
      expect(settled).toBe(true);
    } finally { link.close(); }
  });

  it("reports native host gathering separately from successful candidate signaling without copying candidate contents", async () => {
    clearConnectionDiagnosticHistory();
    let answerCandidate;
    const stood = stand({ signalImpl: async (method) => {
      if (method === "rtc.offer") {
        FakePeerConnection.instances.at(-1).iceGatheringState = "gathering";
        return { sdp: "v=0 answer" };
      }
      if (method === "rtc.ice") return new Promise((resolve) => { answerCandidate = resolve; });
      return {};
    } });
    await tick();
    const peer = stood.peer();
    peer.iceGatheringState = "gathering";
    peer.channels.get("app").open();
    peer.channels.get("term").open();
    peer.connectionState = "connected";
    peer.emit("connectionstatechange");
    const resolved = await stood.link;
    try {
      peer.gather({ type: "host", candidate: "candidate:1 1 udp 1 private-name.local 5000 typ host ufrag private-token" });
      await tick();
      peer.iceGatheringState = "complete";
      peer.gather(null);
      peer.emit("icegatheringstatechange");
      await tick();
      const gathered = connectionDiagnosticHistory().filter(({ event }) => event === "local-candidates").at(-1);
      expect(gathered).toMatchObject({ generation: 0, gathered: { host: 1 }, delivered: { host: 0 } });

      answerCandidate({});
      await tick();
      const delivered = connectionDiagnosticHistory().filter(({ event }) => event === "local-candidates").at(-1);
      expect(delivered).toMatchObject({ generation: 0, gathered: { host: 1 }, delivered: { host: 1 } });
      expect(JSON.stringify([gathered, delivered])).not.toMatch(/private-name|5000|private-token/);
    } finally {
      resolved.close();
    }
  });

  it("bounds initial signaling and removes gathering listeners while the ready connection keeps carrying", async () => {
    vi.useFakeTimers();
    const stood = stand({ openTimeoutMs: 100, signalImpl: async (method) => {
      if (method !== "rtc.offer") return {};
      FakePeerConnection.instances.at(-1).iceGatheringState = "gathering";
      return { sdp: "v=0 answer" };
    } });
    await vi.advanceTimersByTimeAsync(0);
    const peer = stood.peer();
    peer.iceGatheringState = "gathering";
    peer.channels.get("app").open();
    peer.channels.get("term").open();
    peer.connectionState = "connected";
    peer.emit("connectionstatechange");
    const resolved = await stood.link;
    try {
      expect(resolved.whenInitialSignalingComplete).toBeTypeOf("function");
      let finished = false;
      const completion = resolved.whenInitialSignalingComplete().then(() => { finished = true; });
      await vi.advanceTimersByTimeAsync(99);
      expect(finished).toBe(false);
      await vi.advanceTimersByTimeAsync(1);
      await completion;
      expect(peer.listenerCount("icegatheringstatechange")).toBe(0);
      expect(peer.closed).toBe(false);
    } finally {
      resolved.close();
      vi.useRealTimers();
    }
  });

  it("settles initial signaling and removes its gathering listeners when the link closes", async () => {
    const stood = stand({ signalImpl: async (method) => {
      if (method !== "rtc.offer") return {};
      FakePeerConnection.instances.at(-1).iceGatheringState = "gathering";
      return { sdp: "v=0 answer" };
    } });
    await tick();
    const peer = stood.peer();
    peer.iceGatheringState = "gathering";
    peer.channels.get("app").open();
    peer.channels.get("term").open();
    peer.connectionState = "connected";
    peer.emit("connectionstatechange");
    const resolved = await stood.link;
    try {
      expect(resolved.whenInitialSignalingComplete).toBeTypeOf("function");
      const completion = resolved.whenInitialSignalingComplete();
      resolved.close();
      await completion;
      expect(peer.listenerCount("icegatheringstatechange")).toBe(0);
    } finally {
      resolved.close();
    }
  });

  // #31: a relay pair should not win the race on the same network. ICE nominates
  // the first pair that connects and never re-nominates, and the browser is the
  // offerer — so the browser is the controlling agent and this check list is
  // where the race is lost. The only lever a browser has is WHEN a candidate
  // enters a check list, at both doors.
  it("holds relay candidates behind the direct ones, in both directions, while the race is on", async () => {
    vi.useFakeTimers();
    try {
      const { peer: peerOf, signalled, candidateSinks } = stand();
      await vi.advanceTimersByTimeAsync(0);
      const peer = peerOf();
      const rtcIce = () => signalled.filter(([method]) => method === "rtc.ice").map(([, params]) => params.candidate.candidate);

      const host = "candidate:1 1 udp 1 10.0.0.2 5000 typ host";
      const relay = "candidate:2 1 udp 1 203.0.113.9 3478 typ relay raddr 0.0.0.0 rport 0";
      // Gathering order as an agent actually produces it: host first, then the
      // TURN allocation. Nothing is held before a direct candidate has gone
      // through, so a browser with only relay candidates never waits.
      peer.gather({ type: "host", candidate: host, toJSON: () => ({ candidate: host }) });
      peer.gather({ type: "relay", candidate: relay, toJSON: () => ({ candidate: relay }) });
      candidateSinks[0]({ type: "rtc.ice", candidate: { candidate: host, sdpMid: "0" } });
      candidateSinks[0]({ type: "rtc.ice", candidate: { candidate: relay, sdpMid: "0" } });
      await vi.advanceTimersByTimeAsync(0);

      // The direct candidate is on the wire and in the check list; the relay one
      // is in neither, though it was gathered and received before the window ran.
      expect(rtcIce()).toEqual([host]);
      expect(peer.remoteCandidates.map((one) => one.candidate)).toEqual([host]);

      // Nothing is dropped: TURN is what makes a symmetric NAT reachable at all,
      // and a browser that withheld it would turn a billed connection into none.
      await vi.advanceTimersByTimeAsync(1500);

      expect(rtcIce()).toEqual([host, relay]);
      expect(peer.remoteCandidates.map((one) => one.candidate)).toEqual([host, relay]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("stops holding once something is carrying: a late relay candidate is not delayed", async () => {
    vi.useFakeTimers();
    try {
      const stood = stand();
      await vi.advanceTimersByTimeAsync(0);
      const peer = stood.peer();
      peer.channels.get("app").open();
      peer.channels.get("term").open();
      peer.connectionState = "connected";
      peer.emit("connectionstatechange");
      await vi.advanceTimersByTimeAsync(0);
      await stood.link;

      const relay = "candidate:9 1 udp 1 203.0.113.9 3478 typ relay";
      peer.gather({ type: "relay", candidate: relay, toJSON: () => ({ candidate: relay }) });
      await vi.advanceTimersByTimeAsync(0);

      // The race is decided; delaying a relay candidate now buys nothing, and a
      // session that later needs TURN to survive a restart wants all of them.
      expect(stood.signalled.filter(([method]) => method === "rtc.ice")).toHaveLength(1);
    } finally {
      vi.useRealTimers();
    }
  });

  // Which way the connection ended up carrying is the one thing about it the
  // reader cannot see, and the bridge writes the same classification on its
  // own side, so the two ends can be read against each other.
  it("says which path it is carrying on when it lands, and again after a restart", async () => {
    const { peer, resolved, signalled } = await upgrade();
    expect(resolved.transportPath()).toBe("direct");

    // The restart lands on a relayed pair, which is the case this is for: a
    // path that was direct is not the path the restart found.
    peer.localCandidateType = "relay";
    peer.fail();
    await tick();
    peer.channels.get("app").open();
    peer.channels.get("term").open();
    peer.connectionState = "connected";
    peer.emit("connectionstatechange");
    await tick();
    await tick();

    expect(signalled.filter(([method]) => method === "rtc.offer").length).toBeGreaterThan(1);
    expect(resolved.transportPath()).toBe("turn");
  });

  it("says nothing about a path it could not read", async () => {
    const { peer, resolved } = await upgrade();
    peer.getStats = async () => new Map();
    expect(resolved.transportPath(), "the sample it took when it landed still stands").toBe("direct");
  });

  it("rejects when the ICE servers cannot be minted, and never builds a peer", async () => {
    FakePeerConnection.instances.length = 0;
    const failing = openPeerLink({
      signal: async () => ({}),
      fetchIceServers: async () => {
        throw new Error("could not mint ICE servers");
      },
      RTCPeerConnectionImpl: FakePeerConnection,
      onPush: () => () => {},
    });
    await expect(failing).rejects.toThrow("could not mint ICE servers");
    expect(FakePeerConnection.instances).toEqual([]);
  });

  it("rejects a refused offer, drops the peer and tells the bridge to drop its own", async () => {
    const { link, signalled } = stand({
      signalImpl: async (method) => {
        if (method === "rtc.offer") throw new Error("unknown method: rtc.offer");
        return {};
      },
    });
    await expect(link).rejects.toThrow("rtc.offer");
    await tick();
    expect(FakePeerConnection.instances.at(-1).closed).toBe(true);
    expect(signalled.map(([method]) => method)).toContain("rtc.close");
  });

  // The two ways this deadline ends are two of rule 3's seven reasons, and the
  // device strip shows whichever it was. Which failure it was is said HERE,
  // where the failure is, so the error carries it rather than being guessed at
  // from a message: a reason the caller cannot read is a machine reported as
  // "it failed" when it never opened in time.
  // On the test's own clock, because the deadline is the thing under test: on
  // the real one the negotiation races a ten-millisecond timer, and a machine
  // busy enough to lose that race fails this for a reason the peer link has
  // nothing to do with — while never reaching the peer it is asked about.
  it("rejects when the channels never open, and blames the deadline", async () => {
    vi.useFakeTimers();
    try {
      const { link } = stand({ openTimeoutMs: 10 });
      // Far enough to have built the peer and be waiting on its channels, and
      // not so far as to have reached the deadline.
      await vi.advanceTimersByTimeAsync(1);
      expect(FakePeerConnection.instances.at(-1).closed).toBe(false);

      await vi.advanceTimersByTimeAsync(10);
      const refused = await link.catch((error) => error);
      expect(refused.message).toMatch(/open/);
      expect(refused.blockedReason).toBe("timeout");
      expect(FakePeerConnection.instances.at(-1).closed).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it("rejects when the connection fails before the channels open, and blames the connection", async () => {
    const { link, peer } = stand();
    await tick();
    peer().fail();
    const refused = await link.catch((error) => error);
    expect(refused.message).toMatch(/failed/);
    expect(refused.blockedReason).toBe("failed");
  });

  it("restarts ICE with fresh credentials when a live connection fails", async () => {
    const { peer, signalled, fetchIceServers } = await upgrade();
    peer.fail();
    await tick();
    await tick();
    expect(fetchIceServers).toHaveBeenCalledTimes(2);
    const offers = signalled.filter(([method]) => method === "rtc.offer");
    expect(offers).toHaveLength(2);
    expect(offers[1][1].sdp).toContain("restart");
    expect(peer.closed).toBe(false); // the same channels keep carrying
  });

  it("keeps recovery signaling for late host gathering after the restored app is ready", async () => {
    let offers = 0;
    let answerCandidate;
    const onConnected = vi.fn();
    const { peer, resolved } = await upgrade({
      onConnected, confirmCarried: async () => true,
      signalImpl: async (method) => {
        if (method === "rtc.offer") {
          offers += 1;
          if (offers > 1) FakePeerConnection.instances.at(-1).iceGatheringState = "gathering";
          return { sdp: "v=0 answer" };
        }
        if (method === "rtc.ice") return new Promise((resolve) => { answerCandidate = resolve; });
        return {};
      },
    });
    const restored = vi.fn();
    resolved.onRestored(restored);
    peer.fail();
    for (let index = 0; index < 4; index += 1) await tick();
    expect(restored).toHaveBeenCalledOnce();
    expect(resolved.recovery.snapshot().recovering).toBe(false);
    const complete = onConnected.mock.calls.at(-1)[0];
    expect(complete).toBeInstanceOf(Promise);
    let settled = false;
    void complete.then(() => { settled = true; });
    peer.gather({ type: "host", candidate: "candidate:1 1 udp 1 late.local 5000 typ host" });
    peer.iceGatheringState = "complete";
    peer.emit("icegatheringstatechange");
    await tick();
    expect(settled).toBe(false);
    answerCandidate({});
    await complete;
    expect(settled).toBe(true);
    expect(peer.listenerCount("icegatheringstatechange")).toBe(0);
    expect(peer.closed).toBe(false);
    resolved.close();
  });

  it("bounds recovery gathering cleanup without delaying restoration or closing the carried path", async () => {
    let offers = 0;
    const connected = vi.fn();
    const { peer, resolved } = await upgrade({
      openTimeoutMs: 100, onConnected: connected, confirmCarried: async () => true,
      signalImpl: async (method) => {
        if (method === "rtc.offer") {
          offers += 1;
          if (offers > 1) FakePeerConnection.instances.at(-1).iceGatheringState = "gathering";
          return { sdp: "v=0 answer" };
        }
        return {};
      },
    });
    vi.useFakeTimers();
    try {
      const restored = vi.fn();
      resolved.onRestored(restored);
      peer.fail();
      await vi.advanceTimersByTimeAsync(0);
      expect(restored).toHaveBeenCalledOnce();
      const complete = connected.mock.calls.at(-1)[0];
      let settled = false;
      void complete.then(() => { settled = true; });
      await vi.advanceTimersByTimeAsync(99);
      expect(settled).toBe(false);
      await vi.advanceTimersByTimeAsync(1);
      await complete;
      expect(settled).toBe(true);
      expect(peer.listenerCount("icegatheringstatechange")).toBe(0);
      expect(peer.closed).toBe(false);
    } finally {
      resolved.close();
      vi.useRealTimers();
    }
  });

  it("settles recovery gathering cleanup when the carried link closes", async () => {
    let offers = 0;
    const connected = vi.fn();
    const { peer, resolved } = await upgrade({
      onConnected: connected, confirmCarried: async () => true,
      signalImpl: async (method) => {
        if (method === "rtc.offer") {
          offers += 1;
          if (offers > 1) FakePeerConnection.instances.at(-1).iceGatheringState = "gathering";
          return { sdp: "v=0 answer" };
        }
        return {};
      },
    });
    peer.fail();
    for (let index = 0; index < 4; index += 1) await tick();
    const complete = connected.mock.calls.at(-1)[0];
    resolved.close();
    await complete;
    expect(peer.listenerCount("icegatheringstatechange")).toBe(0);
  });

  it("publishes bounded recovery epochs around an in-place ICE restart", async () => {
    const { peer, resolved } = await upgrade();
    const states = [];
    const unsubscribe = resolved.recovery.subscribe((state) => states.push(state));

    peer.fail();
    await tick();
    await tick();

    expect(states).toEqual([
      { epoch: 1, recovering: true },
      { epoch: 2, recovering: false },
    ]);
    expect(resolved.recovery.snapshot()).toEqual({ epoch: 2, recovering: false });
    unsubscribe();
  });

  it("does not report a restart restored while stale channels are open but the peer is not connected", async () => {
    const restored = [];
    const { peer } = await upgrade({ onConnected: () => restored.push("up") });
    peer.autoConnect = false;
    peer.fail();
    await tick();
    await tick();
    expect(restored).toEqual([]);

    peer.connectionState = "connected";
    peer.emit("connectionstatechange");
    await tick();
    expect(restored).toEqual(["up"]);
  });

  it("cancels a pending restart and its deadline when the link closes", async () => {
    vi.useFakeTimers();
    try {
      let offers = 0;
      let finishRestart;
      const stood = stand({
        signalImpl: async (method) => {
          if (method !== "rtc.offer") return {};
          offers += 1;
          if (offers === 1) return { sdp: "v=0 answer" };
          return new Promise((resolve) => { finishRestart = resolve; });
        },
      });
      await vi.advanceTimersByTimeAsync(0);
      const peer = stood.peer();
      peer.channels.get("app").open();
      peer.channels.get("term").open();
      peer.connectionState = "connected";
      peer.emit("connectionstatechange");
      const link = await stood.link;
      peer.fail();
      await vi.advanceTimersByTimeAsync(0);
      link.close();
      await vi.advanceTimersByTimeAsync(0);
      expect(vi.getTimerCount()).toBe(0);
      finishRestart({ sdp: "v=0 late answer" });
      await vi.advanceTimersByTimeAsync(0);
      expect(peer.remoteDescriptions).toHaveLength(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("closes the link when the restart itself fails, so the session falls back", async () => {
    let offers = 0;
    const stood = stand({
      signalImpl: async (method) => {
        if (method !== "rtc.offer") return {};
        offers += 1;
        if (offers > 1) throw new Error("the bridge is gone");
        return { sdp: "v=0 answer" };
      },
    });
    await tick();
    const peer = stood.peer();
    peer.channels.get("app").open();
    peer.channels.get("term").open();
    peer.connectionState = "connected";
    peer.emit("connectionstatechange");
    const { app, term } = await stood.link;
    const lost = [];
    app.onClose(() => lost.push("app"));
    term.onClose(() => lost.push("term"));
    peer.fail();
    await tick();
    await tick();
    expect(peer.closed).toBe(true);
    expect(lost.sort()).toEqual(["app", "term"]);
  });

  // #123: after a bridge restart the ICE restart reaches the NEW process, which
  // answers the offer; ICE and DTLS connect and the channels still read `open`
  // from the dead process's association. A restart is not trusted until the
  // caller confirms the session is carried over it.
  it("closes the link when a restart connects but nothing is carried over it", async () => {
    clearConnectionDiagnosticHistory();
    const restored = [];
    const { peer, resolved } = await upgrade({
      onConnected: () => restored.push("up"),
      confirmCarried: async () => false,
    });
    const lost = [];
    resolved.app.onClose(() => lost.push("app"));
    resolved.term.onClose(() => lost.push("term"));
    peer.fail();
    await vi.waitFor(() => expect(peer.closed).toBe(true));
    expect(lost.sort()).toEqual(["app", "term"]);
    expect(restored).toEqual([]);
    const history = connectionDiagnosticHistory();
    expect(history.find((entry) => entry.event === "restart-failed")?.reason).toBe("not-carried");
    expect(history.filter((entry) => entry.event === "connected").map((entry) => entry.phase)).toEqual(["initial"]);
  });

  it("holds the restart as recovering until the carry is confirmed, then keeps the link", async () => {
    let confirm;
    const restored = [];
    const { peer, resolved } = await upgrade({
      onConnected: () => restored.push("up"),
      confirmCarried: () => new Promise((resolve) => { confirm = resolve; }),
    });
    peer.fail();
    await vi.waitFor(() => expect(confirm).toBeTypeOf("function"));
    // The path probe stands down while this is true; the verdict is this one's.
    expect(resolved.recovery.snapshot().recovering).toBe(true);
    // And the ring says the machine is being reconnected to.
    expect(resolved.restoring()).toBe(true);
    expect(restored).toEqual([]);
    confirm(true);
    await vi.waitFor(() => expect(restored).toEqual(["up"]));
    expect(resolved.recovery.snapshot().recovering).toBe(false);
    expect(resolved.restoring()).toBe(false);
    expect(peer.closed).toBe(false);
  });

  // #130: the failure watcher is latched while the carry check runs, so a path
  // failing again inside it raises no second restart. The link must not then
  // report the restart landed on a failed path.
  it("closes the link when the path fails again while the carry is checked", async () => {
    clearConnectionDiagnosticHistory();
    let confirm;
    const restored = [];
    const { peer, resolved } = await upgrade({
      onConnected: () => restored.push("up"),
      confirmCarried: () => new Promise((resolve) => { confirm = resolve; }),
    });
    resolved.onRestored(() => restored.push("restored"));
    peer.fail();
    await vi.waitFor(() => expect(confirm).toBeTypeOf("function"));
    peer.fail(); // heard by the latched watcher, which starts nothing
    confirm(true);
    await vi.waitFor(() => expect(peer.closed).toBe(true));
    expect(restored).toEqual([]);
    expect(resolved.restoring()).toBe(false);
    const history = connectionDiagnosticHistory();
    expect(history.find((entry) => entry.event === "restart-failed")?.reason).toBe("failed");
    expect(history.filter((entry) => entry.event === "connected").map((entry) => entry.phase)).toEqual(["initial"]);
  });

  // #123: the restart can land on a NEW bridge process that answers it and
  // carries — and holds nothing of this session: no greeting, so no
  // subscriptions and no pushes. Whoever owns the session greets it again.
  it("says a failed path was restored, once the restart carries", async () => {
    const restored = [];
    const { peer, resolved } = await upgrade({ confirmCarried: async () => true });
    resolved.onRestored(() => restored.push("restored"));
    peer.fail();
    await vi.waitFor(() => expect(restored).toEqual(["restored"]));
    expect(peer.closed).toBe(false);
  });

  it("says nothing was restored when the restart carries nothing", async () => {
    const restored = [];
    const { peer, resolved } = await upgrade({ confirmCarried: async () => false });
    resolved.onRestored(() => restored.push("restored"));
    peer.fail();
    await vi.waitFor(() => expect(peer.closed).toBe(true));
    expect(restored).toEqual([]);
  });

  it("closing the link takes both carriers and the peer connection with it", async () => {
    const { peer, resolved, signalled } = await upgrade();
    const lost = [];
    resolved.app.onClose(() => lost.push("app"));
    resolved.term.onClose(() => lost.push("term"));
    resolved.close();
    await tick();
    expect(peer.closed).toBe(true);
    expect(lost.sort()).toEqual(["app", "term"]);
    expect(signalled.map(([method]) => method)).toContain("rtc.close");
  });

  // Rule 4: the rendezvous is closed once both channels are open and reopened
  // when the connection reports `failed`. peerLink says when; what the caller
  // does about it is the caller's.
  it("says when it is connected: once the channels open, and after every restart", async () => {
    const connected = [];
    const { peer } = await upgrade({ onConnected: () => connected.push("up") });
    expect(connected).toEqual([]); // initial relay handoff belongs to the acknowledged app greeting

    peer.fail();
    await tick();
    await tick();
    expect(connected).toEqual(["up"]);
  });

  it("waits for the caller to reopen the rendezvous before it offers a restart", async () => {
    const order = [];
    const { peer, signalled } = await upgrade({
      onFailed: async () => {
        await tick();
        order.push("reopened");
      },
      onConnected: () => order.push("connected"),
    });
    order.length = 0;

    peer.fail();
    await tick();
    await tick();
    await tick();

    expect(order).toEqual(["reopened", "connected"]);
    expect(signalled.filter(([method]) => method === "rtc.offer")).toHaveLength(2);
  });

  it("closes the link when the caller cannot reopen the rendezvous", async () => {
    const { peer, resolved } = await upgrade({
      onFailed: async () => {
        throw new Error("the relay is unreachable");
      },
    });
    const lost = [];
    resolved.app.onClose(() => lost.push("app"));

    peer.fail();
    await tick();
    await tick();

    expect(peer.closed).toBe(true);
    expect(lost).toEqual(["app"]);
  });

  it("takes the bridge's candidates off the push stream and leaves everything else alone", async () => {
    const { peer, candidateSinks } = await upgrade();

    candidateSinks[0]({ type: "entity.changed", id: "run-7" });
    await tick();
    expect(peer.remoteCandidates).toEqual([]);
  });

  it("leaves nothing waiting on the connection it settled", async () => {
    vi.useFakeTimers();
    try {
      const stood = stand();
      await vi.advanceTimersByTimeAsync(0);
      const peer = stood.peer();
      peer.channels.get("app").open();
      peer.channels.get("term").open();
      peer.connectionState = "connected";
      peer.emit("connectionstatechange");
      await stood.link;

      expect(vi.getTimerCount()).toBe(0); // the open deadline is not still ticking
      expect(peer.listenerCount("connectionstatechange")).toBe(2); // restart watcher and diagnostic observer
      for (const channel of peer.channels.values()) expect(channel.listenerCount("open")).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });
});

// #31 point 2: ICE never re-nominates, so a session that lost the race on a
// network where a direct pair also works is billed for TURN for its whole life.
// The only lever after nomination is an ICE restart, which re-runs the race — and
// it is worth the disturbance only where the browser has already PROVED a direct
// pair carries.
describe("a session that landed on a relayed pair", () => {
  it("reports browser-no-host-candidates and keeps TURN when native gathering finishes without a local host", async () => {
    const { peer, resolved, candidateSinks, signalled } = await landedOnRelay({ alsoDirect: null });
    peer.gather({ type: "relay", candidate: "candidate:1 1 udp 1 203.0.113.1 5000 typ relay" });
    peer.iceGatheringState = "complete";
    peer.emit("icegatheringstatechange");
    peer.gather(null);
    candidateSinks[0]({ type: "rtc.diagnostics", event: "mdns-resolved", reason: "direct-checks-succeeded", candidates: { mdns_resolved: 1 }, hostname: "private.local" });
    await vi.advanceTimersByTimeAsync(0);
    expect(connectionDiagnosticHistory().filter((entry) => entry.event === "local-candidates").at(-1))
      .toMatchObject({ reason: "browser-no-host-candidates", gathered: { host: 0 } });
    await vi.advanceTimersByTimeAsync(60000);
    expect(offers(signalled)).toBe(1);
    expect(resolved.transportPath()).toBe("turn");
    expect(peer.closed).toBe(false);
    expect(JSON.stringify(connectionDiagnosticHistory())).not.toMatch(/private.local|203\.0\.113\.1|5000/);
    resolved.close();
  });

  it("does not report absent browser hosts when the local offer already contains a host candidate", async () => {
    const { peer, resolved, candidateSinks, signalled } = await landedOnRelay({
      alsoDirect: null,
      signalImpl: async (method) => {
        if (method === "rtc.offer") {
          const peer = FakePeerConnection.instances.at(-1);
          peer.localDescription = { type: "offer", sdp: "v=0\na=candidate:1 1 udp 1 preexisting.local 5000 typ host\n" };
          return { sdp: "v=0 answer" };
        }
        return {};
      },
    });
    peer.iceGatheringState = "complete";
    peer.emit("icegatheringstatechange");
    peer.gather(null);
    candidateSinks[0]({ type: "rtc.diagnostics", event: "mdns-resolved", reason: "direct-checks-succeeded", candidates: { mdns_resolved: 1 } });
    await vi.advanceTimersByTimeAsync(0);
    expect(connectionDiagnosticHistory().filter((entry) => entry.event === "local-candidates"))
      .not.toContainEqual(expect.objectContaining({ reason: "browser-no-host-candidates" }));
    await vi.advanceTimersByTimeAsync(20000);
    expect(offers(signalled)).toBe(2);
    expect(JSON.stringify(connectionDiagnosticHistory())).not.toMatch(/preexisting.local|5000/);
    resolved.close();
  });
  beforeEach(() => {
    vi.useFakeTimers();
    clearConnectionDiagnosticHistory();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("upgrades when direct checks succeed after the first 20-second sample", async () => {
    const { peer, resolved, signalled } = await landedOnRelay({ alsoDirect: null });
    await vi.advanceTimersByTimeAsync(20000);
    expect(offers(signalled)).toBe(1);
    expect(diagnosticsOf("direct-pair")).toEqual(["none-to-try"]);

    let asked = 0;
    peer.getStats = async () => asReport([
      ...pairEntries("direct", { localType: "host", remoteType: "host", nominated: ++asked > 1 }),
      ...(asked === 1 ? pairEntries("relayed", { localType: "relay", remoteType: "host", nominated: true }) : []),
    ]);
    await vi.advanceTimersByTimeAsync(5000);

    expect(offers(signalled)).toBe(2);
    expect(resolved.transportPath()).toBe("direct");
    expect(diagnosticsOf("direct-pair")).toEqual(["none-to-try", "trying", "renominated"]);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("watches a recovery that changes an initially direct session to TURN", async () => {
    const { peer, resolved, signalled } = await landedDirect();
    peer.getStats = async () => asReport([
      ...pairEntries("relayed", { localType: "relay", remoteType: "host", nominated: true }),
      ...pairEntries("direct", { localType: "host", remoteType: "host" }),
    ]);
    peer.fail();
    await vi.advanceTimersByTimeAsync(0);
    expect(resolved.transportPath()).toBe("turn");
    expect(offers(signalled)).toBe(2);

    await vi.advanceTimersByTimeAsync(20000);
    expect(offers(signalled)).toBe(3);
    expect(diagnosticsOf("direct-pair")).toContain("trying");
  });

  it.each(["neighbor-pressure", "neighbor-snapshot-unavailable"])("upgrades a recovered TURN path after 20 seconds even when its sweep reports %s", async (reason) => {
    const { peer, resolved, signalled, candidateSinks } = await landedDirect();
    const app = resolved.app;
    const term = resolved.term;
    candidateSinks[0](bridgeGenerationDiagnostic(1));
    const relayWithFailedHost = asReport([
      ...pairEntries("relayed", { localType: "relay", remoteType: "host", nominated: true }),
      ...pairEntries("failed-host", { localType: "host", remoteType: "host", state: "failed" }),
    ]);
    peer.getStats = async () => offers(signalled) < 3 ? relayWithFailedHost
      : asReport(pairEntries("fresh-direct", { localType: "host", remoteType: "prflx", nominated: true }));
    peer.iceConnectionState = "failed";
    peer.emit("iceconnectionstatechange");
    peer.fail();
    await vi.advanceTimersByTimeAsync(0);
    expect(offers(signalled)).toBe(2);
    expect(resolved.transportPath()).toBe("turn");
    const reconnectedAt = connectionDiagnosticHistory().find((entry) => entry.event === "connected" && entry.phase === "restart").at;

    // Match the export: the bridge marker repeats 1, and the active sweep has
    // cumulative counters and an already-followed PRFLX. Browser eligibility
    // uses eligible unresolved ports, not the historical probe counters.
    candidateSinks[0](bridgeGenerationDiagnostic(1));
    candidateSinks[0](hostSweepDiagnostic(1, {
      status: "progress", candidateReason: "mdns-unresolved", candidates: { mdns_unresolved: 1 },
      reason, addresses_sent: 80, addresses_attempted: 86, prflx_followed: true,
      destinations_scouted: 651, neighbors_pending_peak: 256,
    }));
    await vi.advanceTimersByTimeAsync(12690);
    expect(diagnosticsOf("direct-pair")).toEqual([]);
    await vi.advanceTimersByTimeAsync(7309);
    expect(offers(signalled)).toBe(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(offers(signalled)).toBe(3);
    expect(resolved.transportPath()).toBe("direct");
    const carrying = connectionDiagnosticHistory().filter((entry) => entry.event === "carrying");
    expect(carrying.map(({ path }) => path)).toEqual(["direct", "turn", "direct"]);
    expect(carrying.at(-1).at - reconnectedAt).toBe(20000);
    expect(connectionDiagnosticHistory().find((entry) => entry.event === "direct-pair"))
      .toMatchObject({ state: "trying", reason: "conntrack-sweep" });
    const exported = JSON.parse(diagnosticsJson(connectionDiagnosticReport()));
    expect(exported.events.filter((entry) => entry.event === "direct-pair")).toMatchObject([
      { at: reconnectedAt + 20000, state: "trying", reason: "conntrack-sweep" },
      { at: reconnectedAt + 20000, state: "renominated", path: "direct" },
    ]);
    expect(peer.localDescriptions.slice(1).every(({ sdp }) => sdp.includes("restart"))).toBe(true);
    expect(resolved.app).toBe(app);
    expect(resolved.term).toBe(term);
    expect(FakePeerConnection.instances).toEqual([peer]);
    expect(peer.closed).toBe(false);
    await vi.advanceTimersByTimeAsync(60000);
    expect(offers(signalled)).toBe(3);
    resolved.close();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("rejects stale recovery sweep evidence at 20 and 25 seconds, then upgrades with fresh evidence at 35 seconds", async () => {
    const { peer, resolved, signalled, candidateSinks } = await landedDirect();
    candidateSinks[0](bridgeGenerationDiagnostic(1));
    candidateSinks[0](hostSweepDiagnostic(1));
    const relayWithFailedHost = asReport([
      ...pairEntries("relayed", { localType: "relay", remoteType: "host", nominated: true }),
      ...pairEntries("failed-host", { localType: "host", remoteType: "host", state: "failed" }),
    ]);
    peer.getStats = async () => offers(signalled) < 3 ? relayWithFailedHost
      : asReport(pairEntries("fresh-direct", { localType: "host", remoteType: "prflx", nominated: true }));
    peer.fail();
    await vi.advanceTimersByTimeAsync(0);
    const reconnectedAt = connectionDiagnosticHistory().find((entry) => entry.event === "connected" && entry.phase === "restart").at;
    candidateSinks[0](bridgeGenerationDiagnostic(2));
    candidateSinks[0](hostSweepDiagnostic(1));
    await vi.advanceTimersByTimeAsync(20000);
    expect(offers(signalled)).toBe(2);
    expect(resolved.transportPath()).toBe("turn");
    candidateSinks[0]({ ...hostSweepDiagnostic(2), generation: 1 });
    await vi.advanceTimersByTimeAsync(5000);
    expect(offers(signalled)).toBe(2);
    expect(diagnosticsOf("direct-pair")).toEqual(["none-to-try"]);

    candidateSinks[0]({ ...hostSweepDiagnostic(2), generation: 2 });
    await vi.advanceTimersByTimeAsync(9999);
    expect(offers(signalled)).toBe(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(offers(signalled)).toBe(3);
    expect(resolved.transportPath()).toBe("direct");
    expect(connectionDiagnosticHistory().filter((entry) => entry.event === "carrying").at(-1).at - reconnectedAt).toBe(35000);
    expect(diagnosticsOf("direct-pair")).toEqual(["none-to-try", "trying", "renominated"]);
    resolved.close();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("rechecks ICE for mDNS resolved after TURN nomination even when old direct checks never succeeded", async () => {
    const { resolved, signalled, candidateSinks } = await landedOnRelay({ alsoDirect: null });
    candidateSinks[0]({ type: "rtc.diagnostics", event: "mdns-resolved", reason: "direct-checks-no-success", candidates: { mdns_resolved: 1 } });
    await vi.advanceTimersByTimeAsync(20000);
    expect(offers(signalled)).toBe(2);
    expect(connectionDiagnosticHistory().find((entry) => entry.event === "direct-pair"))
      .toMatchObject({ state: "trying", reason: "mdns-resolved" });
    await vi.advanceTimersByTimeAsync(60000);
    expect(offers(signalled)).toBe(2);
    expect(resolved.transportPath()).toBe("turn");
  });

  it.each(["failed", "in-progress", null])("uses one fresh restart for eligible unresolved sweep evidence with old browser host pairs %s", async (alsoDirect) => {
    const { peer, resolved, signalled, candidateSinks } = await landedOnRelay({ alsoDirect });
    candidateSinks[0](bridgeGenerationDiagnostic(1));
    candidateSinks[0](hostSweepDiagnostic(1));
    await vi.advanceTimersByTimeAsync(20000);
    expect(offers(signalled)).toBe(2);
    expect(peer.localDescriptions.at(-1).sdp).toContain("restart");
    expect(connectionDiagnosticHistory().find((entry) => entry.event === "direct-pair"))
      .toMatchObject({ state: "trying", reason: "conntrack-sweep" });
    candidateSinks[0](bridgeGenerationDiagnostic(2));
    candidateSinks[0](hostSweepDiagnostic(2));
    await vi.advanceTimersByTimeAsync(60000);
    expect(offers(signalled)).toBe(2);
    resolved.close();
  });

  it.each([
    { eligible: false, status: "skipped", reason: "no-host-socket" },
    { eligible: false, status: "skipped", reason: "subnet-too-large" },
    { eligible: true, status: "skipped", reason: "packet-limit" },
    { candidates: {}, candidateReason: "no-host-candidates" },
    { candidates: { mdns_resolved: 1 }, candidateReason: "direct-checks-no-success" },
    { eligible: false, status: "stopped", reason: "resolved" },
    { eligible: false, status: "stopped", reason: "direct-selected" },
  ])("does not restart without eligible unresolved sweep evidence: %j", async (detail) => {
    const { resolved, signalled, candidateSinks } = await landedOnRelay({ alsoDirect: "failed" });
    candidateSinks[0](bridgeGenerationDiagnostic(1));
    candidateSinks[0](hostSweepDiagnostic(1, detail));
    await vi.advanceTimersByTimeAsync(60000);
    expect(offers(signalled)).toBe(1);
    resolved.close();
  });

  it.each(["nat-evidence-missing", "nat-address-mismatch"])("keeps the same TURN carriers and optional restart budget when %s clears sweep eligibility", async (reason) => {
    const { peer, resolved, signalled, candidateSinks } = await landedOnRelay({ alsoDirect: "failed" });
    const app = resolved.app;
    const term = resolved.term;
    const appChannel = peer.channels.get("app");
    const termChannel = peer.channels.get("term");
    const closed = vi.fn();
    app.onClose(closed);
    term.onClose(closed);
    const envelope = { nonce: "AQID", ciphertext: "BAUG" };
    await app.send(envelope);
    candidateSinks[0](bridgeGenerationDiagnostic(1));
    candidateSinks[0](hostSweepDiagnostic(1, { addresses_sent: 3, scout_datagrams_sent: 12 }));
    candidateSinks[0](hostSweepDiagnostic(1, {
      status: "skipped", reason, eligible: false, eligible_unresolved: 0,
      addresses_sent: 0, addresses_attempted: 0, scout_datagrams_sent: 0, scout_attempted: 0,
    }));

    await vi.advanceTimersByTimeAsync(20000);
    expect(offers(signalled)).toBe(1);
    expect(peer.localDescriptions.filter(({ sdp }) => sdp.includes("restart"))).toEqual([]);
    expect(FakePeerConnection.instances).toEqual([peer]);
    expect(peer.connectionState).toBe("connected");
    expect(peer.closed).toBe(false);
    expect(resolved.transportPath()).toBe("turn");
    expect(resolved.app).toBe(app);
    expect(resolved.term).toBe(term);
    expect(peer.channels.get("app")).toBe(appChannel);
    expect(peer.channels.get("term")).toBe(termChannel);
    expect(appChannel.readyState).toBe("open");
    expect(termChannel.readyState).toBe("open");
    expect(app.peerIsConnected()).toBe(true);
    expect(closed).not.toHaveBeenCalled();
    await app.send(envelope);
    expect(appChannel.sent.map((text) => JSON.parse(text))).toEqual([envelope, envelope]);
    expect(diagnosticsOf("direct-pair")).not.toContain("trying");

    candidateSinks[0](hostSweepDiagnostic(1, { eligible_unresolved: 1 }));
    await vi.advanceTimersByTimeAsync(5000);
    expect(offers(signalled)).toBe(2);
    expect(connectionDiagnosticHistory().filter((entry) => entry.event === "direct-pair" && entry.state === "trying"))
      .toEqual([expect.objectContaining({ reason: "conntrack-sweep" })]);
    resolved.close();
  });

  it.each([
    { eligible: false, status: "skipped", reason: "subnet-too-large", candidates: { mdns_pending: 2 }, eligible_unresolved: 1 },
    { eligible: false, status: "stopped", reason: "window-expired", eligible_unresolved: 1 },
    { eligible: false, status: "stopped", reason: "packet-limit", eligible_unresolved: 1 },
    { eligible: false, status: "stopped", reason: "resolved", candidates: { mdns_pending: 1, mdns_resolved: 1 }, eligible_unresolved: 1 },
  ])("keeps usable unresolved sweep evidence after a later per-port event: %j", async (detail) => {
    const { resolved, signalled, candidateSinks } = await landedOnRelay({ alsoDirect: "failed" });
    candidateSinks[0](bridgeGenerationDiagnostic(1));
    candidateSinks[0](hostSweepDiagnostic(1));
    candidateSinks[0](hostSweepDiagnostic(1, detail));
    await vi.advanceTimersByTimeAsync(20000);
    expect(offers(signalled)).toBe(2);
    expect(connectionDiagnosticHistory().find((entry) => entry.event === "direct-pair"))
      .toMatchObject({ state: "trying", reason: "conntrack-sweep" });
    resolved.close();
  });

  it.each([
    { reason: "resolved", candidates: { mdns_pending: 0, mdns_unresolved: 0 } },
    { reason: "resolved", candidates: { mdns_pending: 1, mdns_resolved: 1 } },
    { reason: "direct-selected", candidates: { mdns_pending: 1 } },
  ])("clears previous sweep upgrade evidence when it is no longer useful: %j", async (detail) => {
    const { resolved, signalled, candidateSinks } = await landedOnRelay({ alsoDirect: "failed" });
    candidateSinks[0](bridgeGenerationDiagnostic(1));
    candidateSinks[0](hostSweepDiagnostic(1));
    candidateSinks[0](hostSweepDiagnostic(1, { ...detail, eligible: false, status: "stopped" }));
    candidateSinks[0](hostSweepDiagnostic(1, { eligible: false, status: "skipped", reason: "subnet-too-large" }));
    await vi.advanceTimersByTimeAsync(20000);
    expect(offers(signalled)).toBe(1);
    resolved.close();
  });

  it("keeps a browser with no gathered host on TURN despite an eligible bridge sweep", async () => {
    const { peer, resolved, signalled, candidateSinks } = await landedOnRelay({ alsoDirect: null });
    peer.gather(null);
    candidateSinks[0](bridgeGenerationDiagnostic(1));
    candidateSinks[0](hostSweepDiagnostic(1));
    await vi.advanceTimersByTimeAsync(20000);
    expect(offers(signalled)).toBe(1);
    expect(connectionDiagnosticHistory().filter((entry) => entry.event === "direct-pair").at(-1))
      .toMatchObject({ state: "none-to-try", reason: "browser-no-host-candidates" });
    resolved.close();
  });

  it("keeps early sweep evidence delivered before initial ICE and DTLS complete", async () => {
    const stood = stand();
    await vi.advanceTimersByTimeAsync(0);
    const peer = stood.peer();
    peer.getStats = async () => asReport([
      ...pairEntries("relayed", { localType: "relay", remoteType: "host", nominated: true }),
      ...pairEntries("failed-direct", { localType: "host", remoteType: "host", state: "failed" }),
    ]);
    stood.candidateSinks[0](bridgeGenerationDiagnostic(1));
    stood.candidateSinks[0](hostSweepDiagnostic(1));
    peer.channels.get("app").open();
    peer.channels.get("term").open();
    peer.connectionState = "connected";
    peer.emit("connectionstatechange");
    const resolved = await stood.link;
    await vi.advanceTimersByTimeAsync(20000);
    expect(offers(stood.signalled)).toBe(2);
    expect(connectionDiagnosticHistory().find((entry) => entry.event === "direct-pair"))
      .toMatchObject({ state: "trying", reason: "conntrack-sweep" });
    resolved.close();
  });

  it("clears sweep evidence when recovery begins and rejects a delayed older sweep after the generation marker", async () => {
    const { peer, resolved, signalled, candidateSinks } = await landedOnRelay({ alsoDirect: "failed" });
    candidateSinks[0](bridgeGenerationDiagnostic(1));
    candidateSinks[0](hostSweepDiagnostic(1));
    peer.fail();
    await vi.advanceTimersByTimeAsync(0);
    expect(offers(signalled)).toBe(2);
    await vi.advanceTimersByTimeAsync(20000);
    expect(offers(signalled)).toBe(2);
    candidateSinks[0](bridgeGenerationDiagnostic(2));
    candidateSinks[0](hostSweepDiagnostic(1));
    await vi.advanceTimersByTimeAsync(5000);
    expect(offers(signalled)).toBe(2);
    candidateSinks[0](hostSweepDiagnostic(2, { candidates: { mdns_unresolved: 1 }, candidateReason: "mdns-unresolved" }));
    await vi.advanceTimersByTimeAsync(10000);
    expect(offers(signalled)).toBe(3);
    resolved.close();
  });

  it("preserves the one sweep upgrade restart budget across multiple failed-path recoveries", async () => {
    const { peer, resolved, signalled, candidateSinks } = await landedOnRelay({ alsoDirect: "failed" });
    candidateSinks[0](bridgeGenerationDiagnostic(1));
    candidateSinks[0](hostSweepDiagnostic(1));
    await vi.advanceTimersByTimeAsync(20000);
    expect(offers(signalled)).toBe(2);
    for (const generation of [3, 4, 5]) {
      peer.fail();
      await vi.advanceTimersByTimeAsync(0);
      candidateSinks[0](bridgeGenerationDiagnostic(generation));
      candidateSinks[0](hostSweepDiagnostic(generation));
      await vi.advanceTimersByTimeAsync(20000);
      expect(offers(signalled)).toBe(generation);
    }
    expect(connectionDiagnosticHistory().filter((entry) => entry.event === "direct-pair" && entry.state === "trying"))
      .toEqual([expect.objectContaining({ reason: "conntrack-sweep" })]);
    resolved.close();
  });

  it("cancels unresolved sweep upgrade evidence when the name resolves in this generation", async () => {
    const { resolved, signalled, candidateSinks } = await landedOnRelay({ alsoDirect: "failed" });
    candidateSinks[0](bridgeGenerationDiagnostic(1));
    candidateSinks[0](hostSweepDiagnostic(1));
    candidateSinks[0]({ type: "rtc.diagnostics", event: "mdns-resolved", reason: "direct-checks-no-success", candidates: { mdns_resolved: 1 } });
    await vi.advanceTimersByTimeAsync(20000);
    expect(offers(signalled)).toBe(2);
    expect(connectionDiagnosticHistory().find((entry) => entry.event === "direct-pair"))
      .toMatchObject({ state: "trying", reason: "mdns-resolved" });
    resolved.close();
  });

  it("keeps resolution evidence arriving between ICE connected and its carrying-path sample", async () => {
    const stood = stand();
    await vi.advanceTimersByTimeAsync(0);
    const peer = stood.peer();
    let finishInitialStats;
    const relay = asReport(pairEntries("relay", { localType: "relay", remoteType: "host", nominated: true }));
    peer.getStats = () => new Promise((resolve) => { finishInitialStats = resolve; });
    peer.channels.get("app").open();
    peer.channels.get("term").open();
    peer.connectionState = "connected";
    peer.emit("connectionstatechange");
    await vi.advanceTimersByTimeAsync(0);
    stood.candidateSinks[0]({ type: "rtc.diagnostics", event: "mdns-resolved", reason: "direct-checks-no-success", candidates: { mdns_resolved: 1 } });
    finishInitialStats(relay);
    const resolved = await stood.link;
    peer.getStats = async () => relay;
    await vi.advanceTimersByTimeAsync(20000);
    expect(offers(stood.signalled)).toBe(2);
    resolved.close();
  });

  it("keeps resolution evidence after ICE nomination while DTLS is still connecting", async () => {
    const stood = stand();
    await vi.advanceTimersByTimeAsync(0);
    const peer = stood.peer();
    peer.localCandidateType = "relay";
    peer.iceConnectionState = "connected";
    peer.connectionState = "connecting";
    peer.emit("iceconnectionstatechange");
    stood.candidateSinks[0]({ type: "rtc.diagnostics", event: "mdns-resolved", reason: "direct-checks-no-success", candidates: { mdns_resolved: 1 } });
    peer.channels.get("app").open();
    peer.channels.get("term").open();
    peer.connectionState = "connected";
    peer.emit("connectionstatechange");
    const resolved = await stood.link;
    await vi.advanceTimersByTimeAsync(20000);
    expect(offers(stood.signalled)).toBe(2);
    resolved.close();
  });

  it("does not reuse pre-recovery mDNS evidence to disturb a fresh relayed path", async () => {
    const { peer, resolved, signalled, candidateSinks } = await landedOnRelay({ alsoDirect: null });
    candidateSinks[0]({ type: "rtc.diagnostics", event: "mdns-resolved", reason: "direct-checks-no-success", candidates: { mdns_resolved: 1 } });
    peer.fail();
    await vi.advanceTimersByTimeAsync(0);
    expect(offers(signalled)).toBe(2);
    await vi.advanceTimersByTimeAsync(30000);
    expect(offers(signalled)).toBe(2);
    resolved.close();
  });

  it("clears queued old resolution evidence at the bridge's ordered new-generation marker", async () => {
    const { peer, resolved, signalled, candidateSinks } = await landedOnRelay({ alsoDirect: null });
    peer.autoConnect = false;
    peer.fail();
    await vi.advanceTimersByTimeAsync(0);
    expect(offers(signalled)).toBe(2);
    candidateSinks[0]({ type: "rtc.diagnostics", event: "mdns-resolved", reason: "direct-checks-no-success", candidates: { mdns_resolved: 1 } });
    candidateSinks[0]({ type: "rtc.diagnostics", event: "generation", reason: "no-host-candidates", candidates: { mdns_resolved: 0 } });
    candidateSinks[0]({ type: "rtc.diagnostics", event: "remote-candidate", reason: "no-host-candidates", candidates: { relay: 1 } });
    peer.connectionState = "connected";
    peer.emit("connectionstatechange");
    await vi.advanceTimersByTimeAsync(30000);
    expect(offers(signalled)).toBe(2);
    expect(connectionDiagnosticHistory().filter((entry) => entry.event === "direct-pair").at(-1))
      .toMatchObject({ state: "none-to-try", reason: "no-host-candidates" });
    resolved.close();
  });

  it("keeps current-generation LAN resolution evidence even before the initial connection lands", async () => {
    const stood = stand();
    await vi.advanceTimersByTimeAsync(0);
    const peer = stood.peer();
    stood.candidateSinks[0]({ type: "rtc.diagnostics", event: "mdns-resolved", reason: "direct-checks-no-success", candidates: { mdns_resolved: 1 } });
    peer.localCandidateType = "relay";
    peer.channels.get("app").open();
    peer.channels.get("term").open();
    peer.connectionState = "connected";
    peer.emit("connectionstatechange");
    const resolved = await stood.link;
    await vi.advanceTimersByTimeAsync(60000);
    expect(offers(stood.signalled)).toBe(2);
    resolved.close();
  });

  it("records safe bridge candidate diagnostics and explains unresolved mDNS", async () => {
    const { candidateSinks, signalled } = await landedOnRelay({ alsoDirect: null });
    candidateSinks[0]({
      type: "rtc.diagnostics", reason: "mdns-unresolved", hostname: "private.local",
      candidates: { host_mdns: 1, mdns_unresolved: 1, extra: "192.168.1.2", relay: -1 },
    });
    await vi.advanceTimersByTimeAsync(20000);
    expect(offers(signalled)).toBe(1);
    expect(connectionDiagnosticHistory().find((entry) => entry.event === "candidate-diagnostics"))
      .toMatchObject({ reason: "mdns-unresolved", candidates: { host_mdns: 1, mdns_unresolved: 1 } });
    expect(connectionDiagnosticHistory().find((entry) => entry.event === "direct-pair"))
      .toMatchObject({ state: "none-to-try", reason: "mdns-unresolved" });
    expect(JSON.stringify(connectionDiagnosticHistory())).not.toMatch(/private.local|192.168.1.2/);
  });

  it("keeps repeated negative samples quiet and drops the monitor when closed", async () => {
    const { resolved, signalled } = await landedOnRelay({ alsoDirect: "failed" });
    await vi.advanceTimersByTimeAsync(60000);
    expect(diagnosticsOf("direct-pair")).toEqual(["none-to-try"]);
    expect(connectionDiagnosticHistory().find((entry) => entry.event === "direct-pair"))
      .toMatchObject({ reason: "direct-checks-failed" });
    resolved.close();
    expect(vi.getTimerCount()).toBe(0);
    await vi.advanceTimersByTimeAsync(60000);
    expect(offers(signalled)).toBe(1);
  });

  it("does not race a recovery negotiation with a late stats response", async () => {
    const { peer, resolved, signalled } = await landedOnRelay({ alsoDirect: null });
    let finishStats;
    peer.getStats = () => new Promise((resolve) => { finishStats = resolve; });
    await vi.advanceTimersByTimeAsync(20000);
    expect(finishStats).toBeTypeOf("function");
    const pendingStats = finishStats;
    peer.autoConnect = false;
    peer.fail();
    await vi.advanceTimersByTimeAsync(0);
    expect(offers(signalled)).toBe(2);
    pendingStats(asReport([
      ...pairEntries("relayed", { localType: "relay", remoteType: "host", nominated: true }),
      ...pairEntries("direct", { localType: "host", remoteType: "host" }),
    ]));
    await vi.advanceTimersByTimeAsync(0);
    expect(offers(signalled)).toBe(2);
    expect(diagnosticsOf("direct-pair")).not.toContain("trying");
    resolved.close();
    await vi.advanceTimersByTimeAsync(0);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("discards viability stats from the previous ICE generation after recovery finishes", async () => {
    const { peer, resolved, signalled } = await landedOnRelay({ alsoDirect: null });
    let finishStats;
    peer.getStats = () => new Promise((resolve) => { finishStats = resolve; });
    await vi.advanceTimersByTimeAsync(20000);
    const pendingStats = finishStats;
    peer.getStats = async () => asReport(pairEntries("relayed", { localType: "relay", remoteType: "host", nominated: true }));
    peer.fail();
    await vi.advanceTimersByTimeAsync(0);
    expect(offers(signalled)).toBe(2);
    expect(resolved.recovery.snapshot().recovering).toBe(false);
    pendingStats(asReport(pairEntries("old-direct", { localType: "host", remoteType: "host" })));
    await vi.advanceTimersByTimeAsync(0);
    expect(offers(signalled)).toBe(2);
    expect(diagnosticsOf("direct-pair")).not.toContain("trying");
    resolved.close();
  });

  it("holds relay candidates behind hosts again during an optional direct restart", async () => {
    const { peer, resolved, signalled, candidateSinks } = await landedOnRelay();
    peer.autoConnect = false;
    peer.iceGatheringState = "gathering";
    await vi.advanceTimersByTimeAsync(20000);
    const host = { type: "host", candidate: "candidate:1 1 udp 1 10.0.0.2 5000 typ host" };
    const relay = { type: "relay", candidate: "candidate:2 1 udp 1 203.0.113.9 5000 typ relay" };
    peer.gather(host);
    peer.gather(relay);
    candidateSinks[0]({ type: "rtc.ice", candidate: host });
    candidateSinks[0]({ type: "rtc.ice", candidate: relay });
    await vi.advanceTimersByTimeAsync(0);
    expect(signalled.filter(([method]) => method === "rtc.ice").map(([, params]) => params.candidate.type)).toEqual(["host"]);
    expect(peer.remoteCandidates).toEqual([host]);
    await vi.advanceTimersByTimeAsync(1500);
    expect(signalled.filter(([method]) => method === "rtc.ice").map(([, params]) => params.candidate.type)).toEqual(["host", "relay"]);
    expect(peer.remoteCandidates).toEqual([host, relay]);
    resolved.close();
  });

  it("keeps the signaling lease through fresh gathering, relay hold release and outbound candidate replies", async () => {
    const order = [];
    const finishCandidate = {};
    const stood = stand({
      onConnected: () => order.push("released-lease"),
      signalImpl: async (method, params) => {
        if (method === "rtc.offer") return { sdp: "v=0 answer" };
        if (method !== "rtc.ice") return {};
        const type = params.candidate.type;
        order.push(`sent-${type}`);
        await new Promise((resolve) => { finishCandidate[type] = resolve; });
        order.push(`replied-${type}`);
        return {};
      },
    });
    await vi.advanceTimersByTimeAsync(0);
    const peer = stood.peer();
    peer.getStats = async () => asReport([
      ...pairEntries("relayed", { localType: "relay", remoteType: "host", nominated: true }),
      ...pairEntries("direct", { localType: "host", remoteType: "host" }),
    ]);
    peer.channels.get("app").open();
    peer.channels.get("term").open();
    peer.connectionState = "connected";
    peer.emit("connectionstatechange");
    const resolved = await stood.link;
    peer.iceGatheringState = "gathering";
    await vi.advanceTimersByTimeAsync(20000);
    expect(order).toEqual([]);
    peer.gather({ type: "host", candidate: "candidate:1 1 udp 1 10.0.0.2 5000 typ host" });
    peer.gather({ type: "relay", candidate: "candidate:2 1 udp 1 203.0.113.9 5000 typ relay" });
    peer.iceGatheringState = "complete";
    peer.emit("icegatheringstatechange");
    await vi.advanceTimersByTimeAsync(0);
    expect(order).toEqual(["sent-host"]);
    finishCandidate.host();
    await vi.advanceTimersByTimeAsync(1500);
    expect(order).toEqual(["sent-host", "replied-host", "sent-relay"]);
    finishCandidate.relay();
    await vi.advanceTimersByTimeAsync(0);
    expect(order).toEqual(["sent-host", "replied-host", "sent-relay", "replied-relay", "released-lease"]);
    resolved.close();
  });

  it("bounds a stalled gathering round and cancels its listeners when the optional attempt ends", async () => {
    const released = vi.fn();
    const stood = stand({ onConnected: released });
    await vi.advanceTimersByTimeAsync(0);
    const peer = stood.peer();
    peer.getStats = async () => asReport([
      ...pairEntries("relayed", { localType: "relay", remoteType: "host", nominated: true }),
      ...pairEntries("direct", { localType: "host", remoteType: "host" }),
    ]);
    peer.channels.get("app").open();
    peer.channels.get("term").open();
    peer.connectionState = "connected";
    peer.emit("connectionstatechange");
    const resolved = await stood.link;
    peer.iceGatheringState = "gathering";
    await vi.advanceTimersByTimeAsync(20000);
    expect(released).not.toHaveBeenCalled();
    expect(peer.listenerCount("icegatheringstatechange")).toBe(1);
    await vi.advanceTimersByTimeAsync(15001);
    expect(released).toHaveBeenCalledTimes(1);
    expect(peer.listenerCount("icegatheringstatechange")).toBe(0);
    expect(peer.closed).toBe(false);
    resolved.close();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("reports a timed-out gathering round as pending when native ICE can still nominate direct later", async () => {
    const { peer, resolved } = await landedOnRelay();
    peer.iceGatheringState = "gathering";
    await vi.advanceTimersByTimeAsync(35001);
    expect(diagnosticsOf("direct-pair")).toEqual(["trying", "pending"]);
    expect(connectionDiagnosticHistory().find((entry) => entry.event === "direct-pair" && entry.state === "pending"))
      .toMatchObject({ reason: "timeout", path: "turn" });
    peer.iceGatheringState = "complete";
    peer.emit("icegatheringstatechange");
    peer.getStats = async () => asReport(pairEntries("direct", { localType: "host", remoteType: "host", nominated: true }));
    await vi.advanceTimersByTimeAsync(5000);
    expect(resolved.transportPath()).toBe("direct");
    expect(diagnosticsOf("direct-pair")).toEqual(["trying", "pending", "renominated"]);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("rejects late fresh-server results after the optional restart timed out without closing TURN", async () => {
    const { peer, resolved, fetchIceServers, signalled } = await landedOnRelay();
    let finishFetch;
    fetchIceServers.mockImplementationOnce(() => new Promise((resolve) => { finishFetch = resolve; }));
    await vi.advanceTimersByTimeAsync(20000);
    expect(finishFetch).toBeTypeOf("function");
    await vi.advanceTimersByTimeAsync(15001);
    expect(resolved.recovery.snapshot().recovering).toBe(false);
    const exported = JSON.parse(diagnosticsJson(connectionDiagnosticReport()));
    expect(exported.events.filter((entry) => entry.event === "direct-pair" && entry.state === "failed"))
      .toMatchObject([{ reason: "timeout", path: "turn" }]);
    finishFetch(SERVERS);
    await vi.advanceTimersByTimeAsync(0);
    expect(offers(signalled)).toBe(1);
    expect(peer.remoteDescriptions).toHaveLength(1);
    expect(peer.closed).toBe(false);
    expect(resolved.transportPath()).toBe("turn");
    resolved.close();
  });

  it("rejects an offer reply arriving after its optional restart timed out without touching the live description", async () => {
    let asked = 0;
    let finishOffer;
    const { peer, resolved, signalled } = await landedOnRelay({
      signalImpl: async (method) => {
        if (method !== "rtc.offer") return {};
        if (++asked === 1) return { sdp: "v=0 answer" };
        return new Promise((resolve) => { finishOffer = resolve; });
      },
    });
    await vi.advanceTimersByTimeAsync(20000);
    expect(offers(signalled)).toBe(2);
    await vi.advanceTimersByTimeAsync(15001);
    expect(resolved.recovery.snapshot().recovering).toBe(false);
    finishOffer({ sdp: "v=0 stale answer" });
    await vi.advanceTimersByTimeAsync(0);
    expect(peer.remoteDescriptions).toEqual([{ type: "answer", sdp: "v=0 answer" }]);
    expect(peer.closed).toBe(false);
    expect(resolved.transportPath()).toBe("turn");
    resolved.close();
  });

  it("reports a direct nomination that settles after the restart first sampled the old relay path", async () => {
    const { peer, resolved, signalled } = await landedOnRelay();
    const moves = [];
    resolved.onPathChanged((path) => moves.push(path));
    await vi.advanceTimersByTimeAsync(20000);
    expect(resolved.transportPath()).toBe("turn");
    expect(offers(signalled)).toBe(2);
    peer.getStats = async () => asReport(pairEntries("direct", { localType: "host", remoteType: "host", nominated: true }));
    await vi.advanceTimersByTimeAsync(5000);
    expect(resolved.transportPath()).toBe("direct");
    expect(moves).toEqual(["direct"]);
    expect(offers(signalled)).toBe(2);
    expect(diagnosticsOf("direct-pair")).toEqual(["trying", "stayed-relayed", "renominated"]);
    const exported = JSON.parse(diagnosticsJson(connectionDiagnosticReport()));
    expect(exported.events.filter((entry) => entry.event === "direct-pair" && entry.state === "stayed-relayed"))
      .toMatchObject([{ path: "turn" }]);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("does not act on a stats response arriving after the link closes", async () => {
    const { peer, resolved, signalled } = await landedOnRelay({ alsoDirect: null });
    let finishStats;
    peer.getStats = () => new Promise((resolve) => { finishStats = resolve; });
    await vi.advanceTimersByTimeAsync(20000);
    resolved.close();
    finishStats(asReport(pairEntries("direct", { localType: "host", remoteType: "host" })));
    await vi.advanceTimersByTimeAsync(0);
    expect(offers(signalled)).toBe(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("tries once for a direct pair, after it has been steady, and says what it landed on", async () => {
    const { peer, signalled, resolved } = await landedOnRelay();
    const before = offers(signalled);

    // Not early: an ICE restart re-gathers and re-checks everything, and a
    // session still settling must not pay for it.
    await vi.advanceTimersByTimeAsync(19000);
    expect(offers(signalled)).toBe(before);

    // The reading the attempt is decided on comes first, then the restart
    // re-nominates and the next reading is the direct pair.
    let asked = 0;
    peer.getStats = async () => {
      asked += 1;
      if (asked === 1) {
        return asReport([
          ...pairEntries("relayed", { localType: "relay", remoteType: "host", nominated: true }),
          ...pairEntries("direct", { localType: "host", remoteType: "host" }),
        ]);
      }
      return asReport(pairEntries("direct", { localType: "host", remoteType: "host", nominated: true }));
    };

    await vi.advanceTimersByTimeAsync(2000);

    expect(offers(signalled)).toBeGreaterThan(before);
    expect(resolved.transportPath()).toBe("direct");
    expect(diagnosticsOf("direct-pair")).toEqual(["trying", "renominated"]);
  });

  // The optional second run at the race is not a reconnect: the path it may
  // leave works, so the ring has nothing to say about it (#123).
  it("is not restoring anything while it tries for a direct pair", async () => {
    const { peer, resolved } = await landedOnRelay();
    const seen = [];
    resolved.recovery.subscribe(() => seen.push(resolved.restoring()));
    peer.getStats = async () => asReport([
      ...pairEntries("relayed", { localType: "relay", remoteType: "host", nominated: true }),
      ...pairEntries("direct", { localType: "host", remoteType: "host" }),
    ]);
    await vi.advanceTimersByTimeAsync(21000);
    expect(diagnosticsOf("direct-pair")[0]).toBe("trying");
    expect(seen.length).toBeGreaterThan(0);
    expect(seen.every((restoring) => restoring === false)).toBe(true);
  });

  it("does not ask for a new greeting after a direct-pair attempt: nothing failed", async () => {
    const { peer, resolved } = await landedOnRelay();
    const restored = [];
    resolved.onRestored(() => restored.push("restored"));
    let asked = 0;
    peer.getStats = async () => {
      asked += 1;
      if (asked === 1) {
        return asReport([
          ...pairEntries("relayed", { localType: "relay", remoteType: "host", nominated: true }),
          ...pairEntries("direct", { localType: "host", remoteType: "host" }),
        ]);
      }
      return asReport(pairEntries("direct", { localType: "host", remoteType: "host", nominated: true }));
    };
    await vi.advanceTimersByTimeAsync(21000);
    expect(diagnosticsOf("direct-pair")).toEqual(["trying", "renominated"]);
    expect(restored).toEqual([]);
  });

  it("does not try when the check list holds no direct pair that worked", async () => {
    // A symmetric NAT, which is what TURN exists for. Disturbing a working
    // relayed path on a guess is worse than paying for it.
    const { signalled } = await landedOnRelay({ alsoDirect: "failed" });
    const before = offers(signalled);

    await vi.advanceTimersByTimeAsync(60000);

    expect(offers(signalled)).toBe(before);
  });

  it("does not try when there is no direct pair in the report at all", async () => {
    const { signalled } = await landedOnRelay({ alsoDirect: null });
    const before = offers(signalled);

    await vi.advanceTimersByTimeAsync(60000);

    expect(offers(signalled)).toBe(before);
  });

  it("asks once and only once, however long the session runs", async () => {
    const { peer, signalled } = await landedOnRelay();
    const before = offers(signalled);

    await vi.advanceTimersByTimeAsync(21000);
    const afterFirst = offers(signalled);
    expect(afterFirst).toBeGreaterThan(before);
    // The restart never becomes usable, so the attempt fails and the session is
    // left where it was.
    peer.connectionState = "connected";
    await vi.advanceTimersByTimeAsync(120000);

    expect(offers(signalled)).toBe(afterFirst);
  });

  it("stays on relay when the attempt does not land, rather than costing the connection", async () => {
    const { peer, resolved, signalled } = await landedOnRelay();

    await vi.advanceTimersByTimeAsync(21000);
    expect(offers(signalled)).toBeGreaterThan(1);
    // The restart times out inside openTimeoutMs and the peer is still connected.
    await vi.advanceTimersByTimeAsync(20000);

    expect(peer.closed).toBe(false);
    expect(resolved.transportPath()).toBe("turn");
  });

  it("tells its owner the path moved, so the ring can redraw the word", async () => {
    // The ring reads the path off the link and repaints on availability and on
    // recovery; a re-nomination under a steady session is neither, so without
    // this it would keep saying "Connected TURN" until something unrelated drew.
    const { peer, resolved } = await landedOnRelay();
    const moves = [];
    resolved.onPathChanged((path) => moves.push(path));

    let asked = 0;
    peer.getStats = async () => {
      asked += 1;
      if (asked === 1) {
        return asReport([
          ...pairEntries("relayed", { localType: "relay", remoteType: "host", nominated: true }),
          ...pairEntries("direct", { localType: "host", remoteType: "host" }),
        ]);
      }
      return asReport(pairEntries("direct", { localType: "host", remoteType: "host", nominated: true }));
    };
    await vi.advanceTimersByTimeAsync(21000);

    expect(moves).toEqual(["direct"]);
  });

  it("says nothing when a restart comes back on the same kind of path", async () => {
    const { resolved } = await landedOnRelay({ alsoDirect: "failed" });
    const moves = [];
    resolved.onPathChanged((path) => moves.push(path));

    await vi.advanceTimersByTimeAsync(60000);

    expect(moves).toEqual([]);
  });

  it("is never asked of a session that is already direct", async () => {
    const { signalled } = await landedDirect();
    const before = offers(signalled);

    await vi.advanceTimersByTimeAsync(60000);

    expect(offers(signalled)).toBe(before);
  });
});
