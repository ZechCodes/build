// Which carrier one session rides, and what runs on every change.
//
// The peer connection is the only thing that ever carries a session (spec rule
// 1: the relay is not a data plane). The other slot is signaling — the
// rendezvous, which carries `rtc.*` and nothing else, and is not a carrier at
// all: a session with no peer is a session nothing is carrying.

import { describe, it, expect, vi } from "vitest";
import { createSessionSwitch } from "../src/core/sessionSwitch.js";

function stand() {
  const rode = [];
  const read = [];
  const onActive = vi.fn();
  const onIdle = vi.fn();
  const carriers = { signal: { name: "signal" }, peer: { name: "peer" }, second: { name: "second" } };
  const carrierSwitch = createSessionSwitch({
    session: { rideOn: (carrier) => rode.push(carrier), readFrom: (carrier) => read.push(carrier) },
    onActive,
    onIdle,
  });
  return { carrierSwitch, rode, read, onActive, onIdle, ...carriers };
}

describe("createSessionSwitch", () => {
  it("carries nothing until the peer opens", () => {
    const { carrierSwitch, rode, onActive, signal } = stand();
    carrierSwitch.signaling(signal);
    expect(carrierSwitch.active()).toBe(null);
    expect(rode).toEqual([]);
    expect(onActive).not.toHaveBeenCalled();
  });

  it("rides the peer the moment it opens, and re-establishes on it", () => {
    const { carrierSwitch, rode, onActive, signal, peer } = stand();
    carrierSwitch.signaling(signal);
    carrierSwitch.peer(peer);
    expect(rode).toEqual([peer]);
    expect(carrierSwitch.active()).toBe(peer);
    expect(onActive).toHaveBeenCalledTimes(1);
  });

  it("is idle the moment the peer leaves, whatever is still signaling", () => {
    const { carrierSwitch, rode, onIdle, signal, peer } = stand();
    carrierSwitch.signaling(signal);
    carrierSwitch.peer(peer);
    carrierSwitch.peer(null);
    expect(onIdle).toHaveBeenCalledTimes(1);
    expect(carrierSwitch.active()).toBe(null);
    expect(rode).toEqual([peer, null]);
  });

  it("says nothing when the signaling wire goes under a live peer", () => {
    const { carrierSwitch, rode, onActive, onIdle, signal, peer } = stand();
    carrierSwitch.signaling(signal);
    carrierSwitch.peer(peer);
    onActive.mockClear();
    carrierSwitch.signaling(null);
    expect(carrierSwitch.active()).toBe(peer);
    expect(rode).toEqual([peer]);
    expect(onActive).not.toHaveBeenCalled();
    expect(onIdle).not.toHaveBeenCalled();
  });

  it("re-establishes on a peer carrier that replaces the old one", () => {
    const { carrierSwitch, rode, onActive, peer, second } = stand();
    carrierSwitch.peer(peer);
    carrierSwitch.peer(second);
    expect(rode).toEqual([peer, second]);
    expect(onActive).toHaveBeenCalledTimes(2);
  });

  it("hands back what re-establishing returns, so a caller can wait for it", async () => {
    const { rode, peer } = stand();
    const carrierSwitch = createSessionSwitch({
      session: { rideOn: (carrier) => rode.push(carrier), readFrom: () => {} },
      onActive: async () => "re-attached",
    });
    await expect(carrierSwitch.peer(peer)).resolves.toBe("re-attached");
  });

  it("reads from every carrier it holds, not only the one that is carrying", () => {
    const { carrierSwitch, read, rode, signal, peer, second } = stand();
    carrierSwitch.signaling(signal);
    carrierSwitch.peer(peer);
    carrierSwitch.signaling(null);
    carrierSwitch.signaling(second); // the rendezvous reopened under a live peer

    // Nothing changed about what carries, so nothing re-established — but the
    // wire the ICE restart rides is one this session reads.
    expect(rode).toEqual([peer]);
    expect(read).toEqual([signal, peer, second]);
  });

  it("stays quiet once closed — a carrier that dies after a deliberate end says nothing", () => {
    const { carrierSwitch, onActive, onIdle, signal, peer } = stand();
    carrierSwitch.signaling(signal);
    carrierSwitch.peer(peer);
    onActive.mockClear();
    carrierSwitch.close();
    carrierSwitch.peer(null);
    carrierSwitch.peer(peer);
    expect(onActive).not.toHaveBeenCalled();
    expect(onIdle).not.toHaveBeenCalled();
    expect(carrierSwitch.active()).toBe(null);
  });
});

// Which wire one call rides: the one routing rule, in the one place that knows
// both slots. `rtc.*` never rides the channel it negotiates; nothing else ever
// rides the rendezvous (rule 1).
describe("the wire one call rides", () => {
  it("puts signaling on the rendezvous and everything else on the peer", () => {
    const { carrierSwitch, signal, peer } = stand();
    carrierSwitch.signaling(signal);
    carrierSwitch.peer(peer);

    expect(carrierSwitch.wireFor("rtc.offer")).toBe(signal);
    expect(carrierSwitch.wireFor("board.list")).toBe(peer);
  });

  // The upgrade is in flight: the user's call is not refused and not sent over
  // the relay, it waits for the channel it is going to ride.
  it("queues a call made before the peer arrives, and answers it with the channel", async () => {
    const { carrierSwitch, peer } = stand();
    const waiting = carrierSwitch.wireFor("board.list");
    expect(waiting).toBeInstanceOf(Promise);

    carrierSwitch.peer(peer);

    await expect(waiting).resolves.toBe(peer);
  });

  it("makes an ICE restart wait for the rendezvous to reopen, rather than failing it", async () => {
    const { carrierSwitch, signal, peer, second } = stand();
    carrierSwitch.signaling(signal);
    carrierSwitch.peer(peer);
    carrierSwitch.signaling(null); // closed once the channels were open

    const waiting = carrierSwitch.wireFor("rtc.offer");
    expect(waiting).toBeInstanceOf(Promise);
    carrierSwitch.signaling(second);

    await expect(waiting).resolves.toBe(second);
  });

  it("fails everything queued when the upgrade fails, rather than holding it for ever", async () => {
    const { carrierSwitch } = stand();
    const call = carrierSwitch.wireFor("board.list");
    const restart = carrierSwitch.wireFor("rtc.offer");

    carrierSwitch.fail(new Error("this device is blocked"));

    await expect(call).rejects.toThrow("this device is blocked");
    await expect(restart).rejects.toThrow("this device is blocked");
  });

  it("refuses a call asked for after the failure, rather than queueing it again", async () => {
    const { carrierSwitch, peer } = stand();
    carrierSwitch.fail(new Error("this device is blocked"));

    await expect(carrierSwitch.wireFor("board.list")).rejects.toThrow("this device is blocked");

    // …until something is carrying again, which is the failure being over.
    carrierSwitch.peer(peer);
    expect(carrierSwitch.wireFor("board.list")).toBe(peer);
  });

  // A device failed closed stays failed closed while its ICE restart
  // negotiates: the rendezvous coming back is not a channel coming back. A
  // user's call held for one would hang out its whole RPC deadline instead of
  // being refused in the words the strip over that machine shows (rule 3).
  it("keeps refusing user calls while only the signaling wire is back", async () => {
    const { carrierSwitch, signal, peer } = stand();
    carrierSwitch.fail(new Error("this device is blocked"));

    carrierSwitch.signaling(signal); // the upgrade reopened the rendezvous to offer a restart

    await expect(carrierSwitch.wireFor("board.list")).rejects.toThrow("this device is blocked");
    expect(carrierSwitch.wireFor("rtc.offer")).toBe(signal); // and the restart rides it

    // Something carrying is what the failure being over means.
    carrierSwitch.peer(peer);
    expect(carrierSwitch.wireFor("board.list")).toBe(peer);
  });

  it("answers a queued call with nothing once the session is closed", async () => {
    const { carrierSwitch } = stand();
    const waiting = carrierSwitch.wireFor("rtc.close");

    carrierSwitch.close();

    await expect(waiting).resolves.toBe(null);
    expect(carrierSwitch.wireFor("rtc.close")).toBe(null);
    expect(carrierSwitch.wireFor("board.list")).toBe(null);
  });
});
