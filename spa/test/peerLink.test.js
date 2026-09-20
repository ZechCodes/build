// The upgrade, whole job in one call: ICE servers, the two negotiated channels,
// the offer and the trickle both ways, settled once both channels are open.
// A failure anywhere leaves the caller on the relay, with no retry loop.

import { describe, it, expect, vi } from "vitest";
import { openPeerLink } from "../src/core/peerLink.js";

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

describe("openPeerLink", () => {
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
