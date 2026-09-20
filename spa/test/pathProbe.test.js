// #30: a session whose path has silently died is not kept for two minutes.
//
// The trigger is an RPC that burned its whole path deadline without even a
// receipt. The verdict is one short ping, and the only thing that saves the
// session is a frame — ICE's word does not, because a path ICE still calls
// connected while SCTP spends 105 seconds retransmitting into it is exactly the
// fault being fixed.

import { describe, it, expect, vi } from "vitest";
import { createPathProbe, PATH_PROBE_EVENT } from "../src/core/pathProbe.js";
import { FRAME_PROOF_OF_LIFE_MS } from "../src/core/pathLiveness.js";
import { clearConnectionDiagnosticHistory, connectionDiagnosticHistory } from "../src/core/connectionDiagnostics.js";

const NOW = 1_000_000;
const pingTimedOut = () => Object.assign(new Error("ping timed out"), { timedOut: true });

/** A probe over a wire a test drives: when frames last arrived, what ICE says,
 *  and whether the ping is going to answer. */
function stand({ frameAt = 0, iceConnected = true, pong = null, wire: given } = {}) {
  const state = { frameAt, iceConnected, now: NOW };
  const wire = given !== undefined ? given : {
    peerFrameAt: () => state.frameAt,
    peerIsConnected: () => state.iceConnected,
  };
  const dead = [];
  const ping = vi.fn(() => (pong ? pong(state) : Promise.resolve({})));
  const probe = createPathProbe({
    ping,
    wire: () => wire,
    rpc: { lastFrameAt: () => 0 },
    onDead: (detail) => dead.push(detail),
    diagnosticId: "dev-a:sess-1",
    now: () => state.now,
  });
  clearConnectionDiagnosticHistory();
  return { probe, ping, dead, state };
}

const records = () => connectionDiagnosticHistory().filter((entry) => entry.event === PATH_PROBE_EVENT);

describe("createPathProbe", () => {
  it("does not ping a path that carried a frame moments ago — busy is not dead", async () => {
    const { probe, ping, dead } = stand({ frameAt: NOW - FRAME_PROOF_OF_LIFE_MS + 1 });

    expect(await probe.judge("thread.post")).toBe("alive");
    expect(ping).not.toHaveBeenCalled();
    expect(dead).toHaveLength(0);
    expect(records()).toMatchObject([{ state: "alive", vouched: "frames" }]);
  });

  it("keeps the session when the pong comes back", async () => {
    const { probe, ping, dead } = stand();

    expect(await probe.judge("thread.post")).toBe("alive");
    expect(ping).toHaveBeenCalledTimes(1);
    expect(dead).toHaveLength(0);
  });

  it("ends the session when the ping times out, though ICE still says connected", async () => {
    const { probe, dead } = stand({ iceConnected: true, pong: () => Promise.reject(pingTimedOut()) });

    expect(await probe.judge("thread.post")).toBe("dead");
    expect(dead).toMatchObject([{ method: "thread.post", vouched: "ice-connected" }]);
  });

  it("keeps the session when a frame arrives while the ping is out", async () => {
    // Something crossed the wire while the ping waited: the path is up and the
    // pong is merely behind a backlog.
    const { probe, dead } = stand({
      pong: (state) => {
        state.frameAt = state.now;
        return Promise.reject(pingTimedOut());
      },
    });

    expect(await probe.judge("thread.post")).toBe("alive");
    expect(dead).toHaveLength(0);
  });

  it("says nothing about a session with no wire under it — the switch owns that", async () => {
    const { probe, ping, dead } = stand({ wire: null });

    expect(await probe.judge("thread.post")).toBe("no-wire");
    expect(ping).not.toHaveBeenCalled();
    expect(dead).toHaveLength(0);
  });

  it("runs one probe at a time: two deadlines together ask once", async () => {
    let release;
    const { probe, ping } = stand({ pong: () => new Promise((resolve) => { release = resolve; }) });

    const first = probe.judge("thread.post");
    const second = probe.judge("board.list");
    release({});

    expect(await first).toBe("alive");
    expect(await second).toBe("alive");
    expect(ping).toHaveBeenCalledTimes(1);
  });

  it("asks again once the first probe has settled", async () => {
    const { probe, ping } = stand();

    await probe.judge("thread.post");
    await probe.judge("board.list");

    expect(ping).toHaveBeenCalledTimes(2);
  });

  it("stops asking once the session is gone", async () => {
    const { probe, ping } = stand();
    probe.stop();

    expect(await probe.judge("thread.post")).toBe("no-wire");
    expect(ping).not.toHaveBeenCalled();
  });

  it("reports a dead path once, however many deadlines fired on it", async () => {
    const { probe, dead } = stand({ pong: () => Promise.reject(pingTimedOut()) });

    expect(await probe.judge("thread.post")).toBe("dead");
    expect(await probe.judge("board.list")).toBe("no-wire");
    expect(dead).toHaveLength(1);
  });

  it("records the probe, what vouched, and the verdict, so the next report carries the timeline", async () => {
    const { probe } = stand({ pong: () => Promise.reject(pingTimedOut()) });

    await probe.judge("thread.post");

    const written = records();
    expect(written.map((entry) => entry.state)).toEqual(["asked", "dead"]);
    expect(written[0]).toMatchObject({ connection: "dev-a:sess-1", method: "thread.post", vouched: "ice-connected" });
    // "ice-connected" in the dead record is the point of keeping it: the
    // report says the ring was still claiming the path when it was judged dead.
    expect(written[1]).toMatchObject({ connection: "dev-a:sess-1", vouched: "ice-connected", refusal: "ping timed out" });
    expect(typeof written[1].waitedMs).toBe("number");
  });
});
