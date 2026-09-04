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
    FakePeerConnection.instances.push(this);
  }
  createDataChannel(label, init) {
    const channel = new FakeChannel(label, init);
    this.channels.set(label, channel);
    return channel;
  }
  async createOffer(options = {}) {
    return { type: "offer", sdp: `v=0 offer ${this.localDescriptions.length}${options.iceRestart ? " restart" : ""}` };
  }
  async setLocalDescription(description) {
    this.localDescriptions.push(description);
  }
  async setRemoteDescription(description) {
    this.remoteDescriptions.push(description);
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
    remoteCandidates: (deliver) => {
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
    candidateSinks[0]({ candidate: "candidate:2 1 udp" });
    await tick();
    expect(peer.remoteCandidates).toEqual([{ candidate: "candidate:2 1 udp" }]);
  });

  it("rejects when the ICE servers cannot be minted, and never builds a peer", async () => {
    FakePeerConnection.instances.length = 0;
    const failing = openPeerLink({
      signal: async () => ({}),
      fetchIceServers: async () => {
        throw new Error("could not mint ICE servers");
      },
      RTCPeerConnectionImpl: FakePeerConnection,
      remoteCandidates: () => () => {},
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

  it("rejects when the channels never open", async () => {
    const { link } = stand({ openTimeoutMs: 10 });
    await expect(link).rejects.toThrow(/open/);
    expect(FakePeerConnection.instances.at(-1).closed).toBe(true);
  });

  it("rejects when the connection fails before the channels open", async () => {
    const { link, peer } = stand();
    await tick();
    peer().fail();
    await expect(link).rejects.toThrow(/failed/);
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
});
