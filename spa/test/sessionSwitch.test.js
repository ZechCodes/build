// Which carrier one session rides, and what runs on every change. The browser
// end of the teardown rule: a session ends when its last carrier is gone.

import { describe, it, expect, vi } from "vitest";
import { createSessionSwitch } from "../src/core/sessionSwitch.js";

function stand() {
  const rode = [];
  const read = [];
  const onActive = vi.fn();
  const onIdle = vi.fn();
  const carriers = { relay: { name: "relay" }, peer: { name: "peer" }, second: { name: "second" } };
  const carrierSwitch = createSessionSwitch({
    session: { rideOn: (carrier) => rode.push(carrier), readFrom: (carrier) => read.push(carrier) },
    onActive,
    onIdle,
  });
  return { carrierSwitch, rode, read, onActive, onIdle, ...carriers };
}

describe("createSessionSwitch", () => {
  it("rides the relay when that is all there is", () => {
    const { carrierSwitch, rode, onActive, relay } = stand();
    carrierSwitch.relay(relay);
    expect(rode).toEqual([relay]);
    expect(carrierSwitch.active()).toBe(relay);
    expect(onActive).toHaveBeenCalledTimes(1);
  });

  it("prefers the peer the moment it opens, and re-establishes on it", () => {
    const { carrierSwitch, rode, onActive, relay, peer } = stand();
    carrierSwitch.relay(relay);
    carrierSwitch.peer(peer);
    expect(rode).toEqual([relay, peer]);
    expect(carrierSwitch.active()).toBe(peer);
    expect(onActive).toHaveBeenCalledTimes(2);
  });

  it("falls back to the relay when the peer is lost", () => {
    const { carrierSwitch, rode, onActive, relay, peer } = stand();
    carrierSwitch.relay(relay);
    carrierSwitch.peer(peer);
    carrierSwitch.peer(null);
    expect(rode).toEqual([relay, peer, relay]);
    expect(onActive).toHaveBeenCalledTimes(3);
  });

  it("says nothing when the relay is lost under a live peer", () => {
    const { carrierSwitch, rode, onActive, onIdle, relay, peer } = stand();
    carrierSwitch.relay(relay);
    carrierSwitch.peer(peer);
    onActive.mockClear();
    carrierSwitch.relay(null);
    expect(carrierSwitch.active()).toBe(peer);
    expect(rode).toEqual([relay, peer]);
    expect(onActive).not.toHaveBeenCalled();
    expect(onIdle).not.toHaveBeenCalled();
  });

  it("is idle only once the last carrier is gone", () => {
    const { carrierSwitch, rode, onIdle, relay, peer } = stand();
    carrierSwitch.relay(relay);
    carrierSwitch.peer(peer);
    carrierSwitch.relay(null);
    expect(onIdle).not.toHaveBeenCalled();
    carrierSwitch.peer(null);
    expect(onIdle).toHaveBeenCalledTimes(1);
    expect(carrierSwitch.active()).toBeNull();
    expect(rode.at(-1)).toBeNull();
  });

  it("hands back what re-establishing returns, so a caller can wait for it", async () => {
    const { rode, relay } = stand();
    const carrierSwitch = createSessionSwitch({
      session: { rideOn: (carrier) => rode.push(carrier), readFrom: () => {} },
      onActive: async () => "re-attached",
    });
    await expect(carrierSwitch.relay(relay)).resolves.toBe("re-attached");
  });

  it("stays quiet once closed — a carrier that dies after a deliberate end says nothing", () => {
    const { carrierSwitch, onActive, onIdle, relay, peer } = stand();
    carrierSwitch.relay(relay);
    carrierSwitch.peer(peer);
    onActive.mockClear();
    carrierSwitch.close();
    carrierSwitch.relay(null);
    carrierSwitch.peer(null);
    carrierSwitch.relay(relay);
    expect(onActive).not.toHaveBeenCalled();
    expect(onIdle).not.toHaveBeenCalled();
    expect(carrierSwitch.active()).toBeNull();
  });

  it("reads from every carrier it holds, not only the one that is carrying", () => {
    const { carrierSwitch, read, rode, relay, peer, second } = stand();
    carrierSwitch.relay(relay);
    carrierSwitch.peer(peer);
    carrierSwitch.relay(null);
    carrierSwitch.relay(second); // the relay is back under a live channel

    // Nothing changed about what carries, so nothing re-established — but the
    // wire signaling is pinned to is one this session reads.
    expect(rode).toEqual([relay, peer]);
    expect(read).toEqual([relay, peer, second]);
  });

  it("routes signaling to the relay and everything else to whatever is carrying", () => {
    const { carrierSwitch, relay, peer } = stand();
    carrierSwitch.relay(relay);
    carrierSwitch.peer(peer);

    expect(carrierSwitch.wireFor("rtc.offer")).toBe(relay);
    expect(carrierSwitch.wireFor("board.list")).toBe(peer);
  });

  it("makes a signaling call wait for the relay to come back, rather than failing it", async () => {
    const { carrierSwitch, relay, peer, second } = stand();
    carrierSwitch.relay(relay);
    carrierSwitch.peer(peer);
    carrierSwitch.relay(null); // the socket went; the link is already reconnecting

    const waiting = carrierSwitch.wireFor("rtc.offer");
    expect(waiting).toBeInstanceOf(Promise);
    carrierSwitch.relay(second);

    await expect(waiting).resolves.toBe(second);
  });

  it("answers a signaling call with nothing once the session is closed", async () => {
    const { carrierSwitch, relay, peer } = stand();
    carrierSwitch.relay(relay);
    carrierSwitch.peer(peer);
    carrierSwitch.relay(null);

    const waiting = carrierSwitch.wireFor("rtc.close");
    carrierSwitch.close();

    await expect(waiting).resolves.toBeNull();
    expect(carrierSwitch.wireFor("rtc.close")).toBeNull();
  });

  it("re-establishes on a fresh relay carrier that replaces the old one", () => {
    const { carrierSwitch, rode, onActive, relay, second } = stand();
    carrierSwitch.relay(relay);
    carrierSwitch.relay(second);
    expect(rode).toEqual([relay, second]);
    expect(onActive).toHaveBeenCalledTimes(2);
  });
});
