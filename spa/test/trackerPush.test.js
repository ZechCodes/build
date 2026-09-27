// Whether a bridge carries the tasks push.
//
// This is load-bearing rather than polite. `KindSet` on the bridge is
// `#[serde(transparent)]` over a `BTreeSet<Kind>`, so an unknown kind fails the
// set, which fails the whole `SubscriptionSpec`, which is `invalid_params` for
// the call — and `addDesired` (core/changeEvents.js) abandons the subscriptions
// queued behind a refused one. Naming `tasks` at a bridge that predates it
// therefore costs that DEVICE every subscription it would have held, not the
// tracker's.
//
// Note the asymmetry it turns on, which is not guessable from the wire: an
// unknown FIELD is dropped silently by the v1 facade, and an unknown KIND
// refuses the call. Two opposite failure modes in one API.

import { describe, expect, it, vi } from "vitest";

let capabilities = null;
vi.mock("../src/core/changeEvents.js", () => ({ bridgeCapabilities: () => capabilities }));

const { carriesTasksPush, inboxPushKinds, tasksPushKinds } = await import("../src/core/trackerPush.js");

/** What a greeted bridge's capabilities look like, with whatever kind list the
 *  case is about. */
const carrying = (kinds) => ({ changes: { subscriptions: true, kinds } });

/** The six a tracker-carrying bridge advertises today. `terminals` arrived from
 *  main while the tracker was being built and `tasks` with the tracker, so the
 *  list has grown twice — which is why nothing here reads its length or its
 *  order. */
const SIX = ["state", "thread", "git", "files", "terminals", "tasks"];

describe("a bridge that carries tasks", () => {
  it("is one whose greeting says so", () => {
    capabilities = carrying(SIX);
    expect(carriesTasksPush("dev-1")).toBe(true);
    expect(tasksPushKinds("dev-1")).toEqual(["tasks"]);
    expect(inboxPushKinds("dev-1")).toEqual(["state", "thread", "tasks"]);
  });

  // The list has grown twice already. A check that reads its length or its last
  // element would have broken on `terminals` without anybody touching it.
  it("is read by asking the list, not by measuring it", () => {
    capabilities = carrying(["tasks"]);
    expect(carriesTasksPush("dev-1")).toBe(true);
    capabilities = carrying([...SIX, "sandwiches"]);
    expect(carriesTasksPush("dev-1")).toBe(true);
  });
});

describe("a bridge that does not", () => {
  it("is asked for no tasks kind at all", () => {
    capabilities = carrying(["state", "thread", "git", "files", "terminals"]);
    expect(carriesTasksPush("dev-1")).toBe(false);
    expect(tasksPushKinds("dev-1")).toEqual([]);
  });

  // The inbox keeps the two kinds it always had. A Tasks tab filling from the
  // ordered pass is a degraded tracker; an inbox with no pushes is a dead
  // client, and one must never be able to cause the other.
  it("keeps state and thread on the inbox subscription", () => {
    capabilities = carrying(["state", "thread"]);
    expect(inboxPushKinds("dev-1")).toEqual(["state", "thread"]);
  });

  // A greeting that states no list is read as carrying nothing: the safe
  // direction, since the cost of guessing wrong is every push on the device.
  it("is what a greeting with no kind list is read as", () => {
    for (const shape of [carrying(undefined), carrying(null), { changes: {} }, {}, null, undefined]) {
      capabilities = shape;
      expect(carriesTasksPush("dev-1")).toBe(false);
      expect(inboxPushKinds("dev-1")).toEqual(["state", "thread"]);
    }
  });
});
