// #31: a relay pair should not win the race on the same network.
//
// ICE nominates the first pair that connects and never re-nominates, and the
// browser is the offerer — so the browser is the controlling agent and the
// browser's check list is where the race is decided. A relay pair often wins it
// because a TURN allocation is gathered and reachable at once while a host pair
// waits on mDNS resolution and consent, and then a session on the same Wi-Fi as
// the machine is billed for TURN egress for its whole life.
//
// The lever a browser actually has is WHEN a candidate enters a check list:
// there is no API for "prefer direct". So relay candidates are held back a
// short window in both directions — ours before we signal them, the bridge's
// before we add them — and a viable direct pair gets that long to win.

import { describe, expect, it, vi } from "vitest";
import {
  RELAY_HOLD_MS,
  candidateTypeOf,
  createRelayHold,
  directPairWorthTrying,
  isRelayCandidate,
} from "../src/core/iceCandidates.js";

/** A local candidate as `icecandidate` gives it: an RTCIceCandidate, which
 *  carries its type as a field. */
const local = (type) => ({ type, candidate: `candidate:1 1 udp 1 10.0.0.2 5000 typ ${type}` });

/** A remote candidate as `rtc.ice` carries it: an RTCIceCandidateInit off the
 *  wire, which is a plain object with an SDP string and no `type` at all. */
const remote = (type) => ({ candidate: `candidate:2 1 udp 1 203.0.113.9 3478 typ ${type} raddr 0.0.0.0 rport 0`, sdpMid: "0" });

describe("candidateTypeOf", () => {
  it("reads the type off a candidate that carries one", () => {
    expect(candidateTypeOf(local("host"))).toBe("host");
    expect(candidateTypeOf(local("relay"))).toBe("relay");
  });

  it("reads it out of the SDP when there is no field — which is every remote one", () => {
    expect(candidateTypeOf(remote("relay"))).toBe("relay");
    expect(candidateTypeOf(remote("srflx"))).toBe("srflx");
    expect(candidateTypeOf(remote("host"))).toBe("host");
  });

  it("prefers the field over the string, so a parse cannot contradict the browser", () => {
    expect(candidateTypeOf({ type: "host", candidate: "candidate:1 1 udp 1 1.2.3.4 1 typ relay" })).toBe("host");
  });

  it("says nothing about a candidate it cannot read rather than guessing", () => {
    expect(candidateTypeOf(null)).toBe("");
    expect(candidateTypeOf({})).toBe("");
    expect(candidateTypeOf({ candidate: "" })).toBe("");
    expect(candidateTypeOf({ candidate: "not a candidate line" })).toBe("");
  });

  it("is not fooled by a candidate whose foundation contains the word", () => {
    expect(candidateTypeOf({ candidate: "candidate:relay 1 udp 1 10.0.0.2 5000 typ host" })).toBe("host");
  });
});

describe("isRelayCandidate", () => {
  it("is only true of a relay candidate", () => {
    expect(isRelayCandidate(local("relay"))).toBe(true);
    expect(isRelayCandidate(remote("relay"))).toBe(true);
    for (const type of ["host", "srflx", "prflx"]) {
      expect(isRelayCandidate(local(type))).toBe(false);
    }
  });

  it("treats a candidate it cannot read as not-relay, so it still reaches the wire", () => {
    expect(isRelayCandidate(null)).toBe(false);
    expect(isRelayCandidate({})).toBe(false);
  });
});

describe("createRelayHold", () => {
  /** A hold with the clock in the test's hands. */
  const stand = () => {
    const delivered = [];
    let fire = null;
    const hold = createRelayHold({
      deliver: (candidate) => delivered.push(candidateTypeOf(candidate)),
      setTimer: (fn) => {
        fire = fn;
        return 1;
      },
      clearTimer: () => {
        fire = null;
      },
    });
    return { hold, delivered, elapse: () => fire?.(), armed: () => fire !== null };
  };

  it("lets a direct candidate straight through", () => {
    const { hold, delivered } = stand();

    hold.offer(local("host"));
    hold.offer(local("srflx"));

    expect(delivered).toEqual(["host", "srflx"]);
  });

  it("holds a relay candidate back until the window is up, and a direct one never waits", () => {
    const { hold, delivered, elapse } = stand();

    hold.offer(local("host"));
    hold.offer(local("relay"));
    expect(delivered).toEqual(["host"]);

    elapse();

    expect(delivered).toEqual(["host", "relay"]);
  });

  it("keeps held relay candidates in the order they were gathered", () => {
    const { hold, delivered, elapse } = stand();

    hold.offer(local("host"));
    hold.offer(local("relay"));
    hold.offer(local("srflx"));
    hold.offer(local("relay"));
    elapse();

    expect(delivered).toEqual(["host", "srflx", "relay", "relay"]);
  });

  it("lets relay through at once once the window is over, and arms no second one", () => {
    const { hold, delivered, elapse, armed } = stand();

    hold.offer(local("host"));
    hold.offer(local("relay"));
    elapse();
    expect(delivered).toEqual(["host", "relay"]);
    expect(armed()).toBe(false);

    // A late relay candidate — a second TURN server answering — is not worth
    // another window: the direct pairs have had theirs.
    hold.offer(local("relay"));

    expect(delivered).toEqual(["host", "relay", "relay"]);
    expect(armed()).toBe(false);
  });

  it("holds nothing back from a browser with no direct candidate to offer", () => {
    // `iceTransportPolicy: "relay"`, or an agent that gathered nothing usable.
    // There is no direct pair for a relay candidate to beat, so making this wait
    // would put the whole cost of #31 on exactly the people TURN exists for.
    const { hold, delivered, armed } = stand();

    hold.offer(local("relay"));
    hold.offer(local("relay"));

    expect(delivered).toEqual(["relay", "relay"]);
    expect(armed()).toBe(false);
  });

  it("starts holding as soon as there is a direct candidate to protect", () => {
    const { hold, delivered, elapse } = stand();

    hold.offer(local("relay"));
    hold.offer(local("host"));
    hold.offer(local("relay"));
    expect(delivered).toEqual(["relay", "host"]);

    elapse();

    expect(delivered).toEqual(["relay", "host", "relay"]);
  });

  it("gives up the wait the moment a direct pair has connected — the race is over", () => {
    const { hold, delivered, armed } = stand();

    hold.offer(local("host"));
    hold.offer(local("relay"));
    hold.stopHolding();

    expect(delivered).toEqual(["host", "relay"]);
    expect(armed()).toBe(false);
  });

  it("drops what it is holding when the connection goes, and delivers nothing after", () => {
    const { hold, delivered, elapse } = stand();

    hold.offer(local("host"));
    hold.offer(local("relay"));
    hold.close();
    elapse();
    hold.offer(local("relay"));

    expect(delivered).toEqual(["host"]);
  });

  it("waits the window the spec names, which is the bridge's own default", () => {
    expect(RELAY_HOLD_MS).toBe(1500);
  });

  it("uses a real timer when none is injected", () => {
    vi.useFakeTimers();
    try {
      const delivered = [];
      const hold = createRelayHold({ deliver: (candidate) => delivered.push(candidateTypeOf(candidate)) });
      hold.offer(local("host"));
      hold.offer(local("relay"));
      expect(delivered).toEqual(["host"]);

      vi.advanceTimersByTime(RELAY_HOLD_MS);

      expect(delivered).toEqual(["host", "relay"]);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("directPairWorthTrying", () => {
  const pair = (id, over = {}) => ({
    type: "candidate-pair",
    id,
    state: "succeeded",
    localCandidateId: `${id}-l`,
    remoteCandidateId: `${id}-r`,
    ...over,
  });
  const candidates = (id, localType, remoteType) => [
    { type: "local-candidate", id: `${id}-l`, candidateType: localType },
    { type: "remote-candidate", id: `${id}-r`, candidateType: remoteType },
  ];

  it("is true when a direct pair has already succeeded beside the relay one we are on", () => {
    // Zech's case exactly: both pairs are viable on the same Wi-Fi and the relay
    // one merely answered first. The browser has already PROVEN the direct pair
    // works, which is what makes an ICE restart worth the disturbance.
    const stats = [
      pair("relayed", { nominated: true }),
      ...candidates("relayed", "relay", "host"),
      pair("direct"),
      ...candidates("direct", "host", "host"),
    ];

    expect(directPairWorthTrying(stats)).toBe(true);
  });

  it("is false when the only direct pair failed its checks", () => {
    const stats = [
      pair("relayed", { nominated: true }),
      ...candidates("relayed", "relay", "host"),
      pair("direct", { state: "failed" }),
      ...candidates("direct", "host", "host"),
    ];

    expect(directPairWorthTrying(stats)).toBe(false);
  });

  it("is false when there is no direct pair at all — a symmetric NAT has nothing to offer", () => {
    const stats = [pair("relayed", { nominated: true }), ...candidates("relayed", "relay", "relay")];

    expect(directPairWorthTrying(stats)).toBe(false);
  });

  it("does not count the pair we are already on", () => {
    const stats = [pair("direct", { nominated: true }), ...candidates("direct", "host", "host")];

    expect(directPairWorthTrying(stats)).toBe(false);
  });

  it("says no when the report cannot be read, rather than restarting on a guess", () => {
    expect(directPairWorthTrying(null)).toBe(false);
    expect(directPairWorthTrying([])).toBe(false);
    expect(directPairWorthTrying([pair("direct")])).toBe(false); // no candidates to read
  });
});
