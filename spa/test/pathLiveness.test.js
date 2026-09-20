// The one rule both sessions on a peer connection read a path's liveness by.
//
// The app session and the terminals share a peer and must not reach different
// answers about whether it is carrying (issue #30). What they may differ on is
// what the answer is WORTH, which is each caller's own and tested with it.

import { describe, it, expect } from "vitest";
import {
  FRAME_PROOF_OF_LIFE_MS,
  PING_TIMEOUT_MS,
  peerFrameAt,
  whatVouchesFor,
} from "../src/core/pathLiveness.js";

const rpcAt = (at) => ({ lastFrameAt: () => at });
const wireAt = (at, connected = false) => ({
  peerFrameAt: () => at,
  peerIsConnected: () => connected,
});

describe("peerFrameAt", () => {
  it("takes whichever channel of the peer carried last", () => {
    expect(peerFrameAt(rpcAt(500), wireAt(900))).toBe(900);
    expect(peerFrameAt(rpcAt(900), wireAt(500))).toBe(900);
  });

  it("reads a wire that cannot answer as silence rather than raising", () => {
    expect(peerFrameAt(rpcAt(700), null)).toBe(700);
    expect(peerFrameAt(null, {})).toBe(0);
    expect(peerFrameAt(null, null)).toBe(0);
  });
});

describe("whatVouchesFor", () => {
  it("names frames when either channel carried inside the proof window", () => {
    const now = 100_000;
    expect(whatVouchesFor(rpcAt(now - 1), wireAt(0), now)).toBe("frames");
    expect(whatVouchesFor(rpcAt(0), wireAt(now - FRAME_PROOF_OF_LIFE_MS + 1), now)).toBe("frames");
  });

  it("prefers frames over ICE: something crossing beats something claiming", () => {
    const now = 100_000;
    expect(whatVouchesFor(rpcAt(now), wireAt(now, true), now)).toBe("frames");
  });

  it("falls back to ICE's word once the frames are stale", () => {
    const now = 100_000;
    const stale = now - FRAME_PROOF_OF_LIFE_MS;
    expect(whatVouchesFor(rpcAt(stale), wireAt(stale, true), now)).toBe("ice-connected");
    expect(whatVouchesFor(rpcAt(stale), wireAt(stale, false), now)).toBe("nothing");
  });

  it("says nothing vouches for a wire that is not there at all", () => {
    expect(whatVouchesFor(rpcAt(0), null, 100_000)).toBe("nothing");
  });

  it("gives a probe less than the proof window, so a pong cannot outlive it", () => {
    expect(PING_TIMEOUT_MS).toBeLessThan(FRAME_PROOF_OF_LIFE_MS);
  });
});
