import { describe, expect, it, vi } from "vitest";
import { compare, parse, satisfies } from "../src/core/bridgeApi/semver.js";
import { ApiError, normalizeError, selectAdapter } from "../src/core/bridgeApi/index.js";
import * as v1 from "../src/core/bridgeApi/v1/index.js";

const greetingV1 = (over = {}) => ({
  api_version: "1.2.0",
  push_events: true,
  events: ["board.changed", "entity.changed", "changes"],
  changes: {
    subscriptions: true,
    mode: "subscriptions",
    kinds: ["state", "thread", "git", "files"],
    batch_ms: { min: 1000, max: 600000 },
  },
  ...over,
});

describe("semver", () => {
  it("parses a full version", () => {
    expect(parse("1.2.3")).toEqual({ major: 1, minor: 2, patch: 3, prerelease: "" });
  });

  it("fills the parts a short version omits", () => {
    expect(parse("2")).toEqual({ major: 2, minor: 0, patch: 0, prerelease: "" });
    expect(parse("1.4")).toEqual({ major: 1, minor: 4, patch: 0, prerelease: "" });
  });

  it("keeps a prerelease tag aside and drops build metadata", () => {
    expect(parse("1.2.3-rc.1+abc")).toEqual({ major: 1, minor: 2, patch: 3, prerelease: "rc.1" });
  });

  it("returns null for what is not a version", () => {
    expect(parse("")).toBe(null);
    expect(parse("v-one")).toBe(null);
    expect(parse(null)).toBe(null);
    expect(parse({ major: 1 })).toBe(null);
  });

  it("orders by major, then minor, then patch", () => {
    expect(compare("1.0.0", "1.0.0")).toBe(0);
    expect(compare("1.0.0", "1.1.0")).toBe(-1);
    expect(compare("2.0.0", "1.9.9")).toBe(1);
    expect(compare("1.0.1", "1.0.0")).toBe(1);
  });

  it("sorts a prerelease below its release", () => {
    expect(compare("1.1.0-rc.1", "1.1.0")).toBe(-1);
    expect(compare("1.1.0", "1.1.0-rc.1")).toBe(1);
  });

  it("satisfies a two-comparator range", () => {
    const range = ">=1.0.0 <2.0.0";
    expect(satisfies("1.0.0", range)).toBe(true);
    expect(satisfies("1.1.0", range)).toBe(true);
    expect(satisfies("1.99.7", range)).toBe(true);
    expect(satisfies("2.0.0", range)).toBe(false);
    expect(satisfies("0.0.0", range)).toBe(false);
  });

  it("understands the other comparators, a bare version, and *", () => {
    expect(satisfies("1.2.3", "*")).toBe(true);
    expect(satisfies("1.2.3", "1.2.3")).toBe(true);
    expect(satisfies("1.2.4", "=1.2.3")).toBe(false);
    expect(satisfies("1.2.3", "<=1.2.3 >1.0.0")).toBe(true);
  });

  it("is false, never a throw, for junk on either side", () => {
    expect(satisfies("nope", ">=1.0.0 <2.0.0")).toBe(false);
    expect(satisfies("1.0.0", "")).toBe(false);
    expect(satisfies("1.0.0", ">=banana")).toBe(false);
  });
});

describe("adapter selection", () => {
  const v2 = { major: 2, range: ">=2.0.0 <3.0.0", create: () => ({}) };

  it("a 1.1 bridge greeting a 1.1-aware SPA: every capability on", () => {
    const selected = selectAdapter(greetingV1());
    expect(selected.major).toBe(1);
    expect(selected.create(vi.fn()).capabilities).toEqual({
      // The kinds come through as the greeting states them: a caller asks
      // whether this bridge carries the one it is about to name, because every
      // kind in one subscribe shares that call's fate.
      changes: { subscriptions: true, kinds: ["state", "thread", "git", "files"] },
      requests: { priority: true },
      errors: { codes: true },
      // The hunks-per-file read arrived in 1.4; a 1.1 bridge is asked for
      // whole patches, and files on an issue arrived in 1.8.
      diffs: { perFile: false },
      // Watching arrived in 1.9 (#64); a 1.1 bridge carries none.
      issues: { attachments: false, watching: false, context: false },
      // A conversation's own compaction threshold arrived in 1.10.
      conversations: { settings: false },
      messages: { context: false },
      threads: { postOperations: false },
    });
  });

  /** Hunks per file is 1.4, and the greeting says nothing about it — so the
   *  minor is the whole of the answer. A surface that asked a 1.3 bridge for
   *  them would draw a stack of files that never load. */
  it("reads hunks per file off the minor alone, from 1.4", () => {
    const greetingAt = (version) => ({
      api_version: version,
      push_events: true,
      changes: { subscriptions: true, kinds: ["git"], items: "bodies" },
    });
    const perFile = (version) =>
      selectAdapter(greetingAt(version)).create(vi.fn()).capabilities.diffs.perFile;

    expect(perFile("1.3.0")).toBe(false);
    expect(perFile("1.4.0")).toBe(true);
    expect(perFile("1.5.2")).toBe(true);
  });

  /** Watching (#64, 1.9) is stated outright the way attachments are, because a
   *  switch wired to a verb the bridge has never heard of can only refuse. The
   *  minor answers for a bridge that has the verbs but predates the flag, and a
   *  bridge that states `false` is taken at its word however new it is. */
  it("reads watching off the greeting's flag, and off the minor from 1.9", () => {
    const greetingAt = (version, over = {}) => ({
      api_version: version,
      push_events: true,
      changes: { subscriptions: true, kinds: ["issues"], items: "bodies" },
      ...over,
    });
    const watching = (version, over) =>
      selectAdapter(greetingAt(version, over)).create(vi.fn()).capabilities.issues.watching;

    // The minor alone, for a bridge that says nothing about it.
    expect(watching("1.8.0")).toBe(false);
    expect(watching("1.9.0")).toBe(true);
    expect(watching("1.10.0")).toBe(true);

    // Stated outright, which outranks the minor in both directions.
    expect(watching("1.8.0", { issues: { watching: true } })).toBe(true);
    expect(watching("1.9.0", { issues: { watching: false } })).toBe(false);

    // Anything that is not a boolean is not a claim, so the minor decides.
    expect(watching("1.8.0", { issues: { watching: "yes" } })).toBe(false);
    expect(watching("1.9.0", { issues: { watching: null } })).toBe(true);
  });

  /** `conversation.settings` (1.10) is stated nowhere in the greeting, so the
   *  minor is the whole of the answer, as it is for hunks per file. */
  it("reads conversation settings off the minor alone, from 1.10", () => {
    const settings = (version) =>
      selectAdapter({ api_version: version, push_events: true }).create(vi.fn()).capabilities.conversations.settings;

    expect(settings("1.9.0")).toBe(false);
    expect(settings("1.10.0")).toBe(true);
    expect(settings("1.11.2")).toBe(true);
  });

  // A greeting's kind list rides through whole, whatever is on it. The list
  // has already grown twice — `terminals` from main, `issues` with the tracker
  // — so nothing reads its length or its order, and a caller asks it whether
  // the one kind it is about to subscribe to is there.
  //
  // Whether the version a bridge reports is one this build serves at all is
  // not asserted here: that is a question about the shared contract fixture,
  // and naming a version in this file would be a copy of the bridge's own
  // constant. apiContract.test.js reads `versions.json` and asks it there.
  it("carries a greeting's kinds through, however many there are", () => {
    const selected = selectAdapter(greetingV1({
      changes: { subscriptions: true, kinds: ["state", "thread", "git", "files", "terminals", "issues"] },
    }));
    expect(selected.create(vi.fn()).capabilities.changes.kinds).toContain("issues");
  });

  it("a 1.1 bridge and a 1.0-only SPA: the 1.0 adapter still serves it", () => {
    const onlyV1 = [{ major: 1, range: ">=1.0.0 <2.0.0", create: () => ({ stale: true }) }];
    const selected = selectAdapter(greetingV1(), onlyV1);
    expect(selected.unsupported).toBe(undefined);
    expect(selected.create(vi.fn())).toEqual({ stale: true });
  });

  it("a 2.x bridge with only a v1 adapter: the app is the one to update", () => {
    expect(selectAdapter({ api_version: "2.3.1" })).toEqual({ unsupported: "app", version: "2.3.1" });
  });

  it("a 1.x bridge with only a v2 adapter: the bridge is the one to update", () => {
    expect(selectAdapter(greetingV1(), [v2])).toEqual({ unsupported: "bridge", version: "1.2.0" });
  });

  // The cache-first client reads bodies off the push and polls nothing, which
  // a 1.1 bridge does not carry. It is not a degraded 1.1 client; it is a gate.
  it("a 1.1 bridge against this SPA: the bridge is the one to update", () => {
    expect(selectAdapter(greetingV1({ api_version: "1.1.0" }))).toEqual({ unsupported: "bridge", version: "1.1.0" });
    expect(selectAdapter(greetingV1({ api_version: "1.0.0" }))).toEqual({ unsupported: "bridge", version: "1.0.0" });
  });

  it("a greeting with no api_version is 0.0.0 on the v1 adapter, every flag false", () => {
    for (const greeting of [null, undefined, {}, { push_events: true }]) {
      const selected = selectAdapter(greeting);
      expect(selected.version).toBe("0.0.0");
      expect(selected.major).toBe(1);
      expect(selected.create(vi.fn()).capabilities).toEqual({
        changes: { subscriptions: false, kinds: [] },
        requests: { priority: false },
        errors: { codes: false },
        diffs: { perFile: false },
        issues: { attachments: false, watching: false, context: false },
        conversations: { settings: false },
        messages: { context: false },
        threads: { postOperations: false },
      });
    }
  });

  it("a 0.0.0 bridge that somehow claims subscriptions is still given none", () => {
    const selected = selectAdapter({ changes: { subscriptions: true } });
    expect(selected.create(vi.fn()).capabilities.changes.subscriptions).toBe(false);
  });
});

describe("named capabilities", () => {
  const features = [
    "changes.subscriptions", "requests.priority", "errors.codes", "diffs.perFile",
    "issues.attachments", "issues.watching", "issues.context", "conversations.settings",
    "messages.context", "threads.postOperations",
  ];
  const flags = (greeting) => {
    const capabilities = selectAdapter(greeting).create(async () => ({})).capabilities;
    return features.filter((name) => {
      const [group, feature] = name.split(".");
      return capabilities[group][feature];
    });
  };
  const namedGreeting = (capabilities) => ({
    api_version: "1.22.0", capabilities,
    // Old fields cannot override the list, in either direction.
    changes: { subscriptions: true, kinds: ["issues"] },
    requests: { priority: true }, errors: { codes: true },
    issues: { attachments: true, watching: true },
  });

  it("maps a 1.19 bridge without a list through the historical table", () => {
    expect(flags({ api_version: "1.19.0" })).toEqual(features.filter((name) => !["messages.context", "threads.postOperations"].includes(name)));
    expect(flags({
      api_version: "1.19.0", message_context: { version: 1 },
      thread_post_operations: { version: 1, status_method: "thread.operation" },
    })).toEqual(features);
  });

  it.each(features)("switches %s on and off independently", (feature) => {
    expect(flags(namedGreeting([feature]))).toEqual([feature]);
    expect(flags(namedGreeting(features.filter((name) => name !== feature))))
      .toEqual(features.filter((name) => name !== feature));
  });

  it("uses names even when legacy booleans deny a feature", () => {
    expect(flags({
      ...namedGreeting(features),
      changes: { subscriptions: false }, requests: { priority: false },
      errors: { codes: false }, issues: { attachments: false, watching: false, context: false },
    })).toEqual(features);
  });

  it("ignores unknown names and malformed list members", () => {
    expect(flags(namedGreeting(["future.feature", null, 10, {}, "issues.attachments"])))
      .toEqual(["issues.attachments"]);
  });

  it("never falls back for absent, empty or malformed lists on new bridges", () => {
    for (const version of ["1.22.0", "1.23.0", "1.99.0"]) {
      for (const capabilities of [undefined, [], null, {}, "issues.attachments"]) {
        const greeting = { ...namedGreeting(capabilities), api_version: version };
        if (capabilities === undefined) delete greeting.capabilities;
        expect(flags(greeting)).toEqual([]);
      }
    }
  });

  it("prefers a list on an older bridge too", () => {
    expect(flags({ ...namedGreeting(["issues.watching"]), api_version: "1.19.0" }))
      .toEqual(["issues.watching"]);
  });

  it("keeps kind metadata and push events separate from feature flags", () => {
    const greeting = { ...namedGreeting(["changes.subscriptions"]), events: ["changes", "future.event"] };
    const adapter = selectAdapter(greeting).create(async () => ({}));
    expect(adapter.capabilities.changes.kinds).toEqual(["issues"]);
    expect(adapter.events).toEqual(greeting.events);
    expect(v1.capabilitiesOf(namedGreeting([])).changes.kinds).toEqual([]);
  });
});

describe("the v1 adapter", () => {
  it("declares a range that starts where pushes carry bodies", () => {
    expect(v1.range).toBe(">=1.2.0 <2.0.0");
    expect(satisfies("1.1.0", v1.range)).toBe(false);
    expect(satisfies("1.2.0", v1.range)).toBe(true);
    expect(satisfies("1.9.4", v1.range)).toBe(true);
  });

  it("passes a call through and returns its result", async () => {
    const call = vi.fn(async () => ({ items: [] }));
    const adapter = v1.create(call, greetingV1());
    await expect(adapter.call("board.list", {})).resolves.toEqual({ items: [] });
    expect(call).toHaveBeenCalledWith("board.list", {});
  });

  it("names the events the greeting names, and the legacy pair when it names none", () => {
    expect(v1.create(vi.fn(), greetingV1()).events).toEqual([
      "board.changed",
      "entity.changed",
      "changes",
    ]);
    expect(v1.create(vi.fn(), { api_version: "1.0.0", push_events: true }).events).toEqual([
      "board.changed",
      "entity.changed",
    ]);
    expect(v1.create(vi.fn(), { api_version: "1.0.0" }).events).toEqual([]);
  });

  it("turns a coded rejection into an ApiError carrying code, retryable and details", async () => {
    const rejection = Object.assign(new Error("queue is full"), {
      error_code: "busy",
      retryable: true,
      details: { depth: 256 },
    });
    const adapter = v1.create(vi.fn(async () => { throw rejection; }), greetingV1());
    const error = await adapter.call("board.list", {}).catch((e) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect(error.code).toBe("busy");
    expect(error.retryable).toBe(true);
    expect(error.details).toEqual({ depth: 256 });
    expect(error.message).toBe("queue is full");
  });

  it("turns a v1.0 bridge's string error into code unknown, message intact", async () => {
    const adapter = v1.create(vi.fn(async () => { throw new Error("issue has no active implementation"); }), {
      api_version: "1.0.0",
    });
    const error = await adapter.call("issue.diff", {}).catch((e) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect(error.code).toBe("unknown");
    expect(error.retryable).toBe(false);
    expect(error.details).toEqual({});
    expect(error.message).toBe("issue has no active implementation");
  });

  it("throws an ApiError when a raw refusal reply is resolved instead of thrown", async () => {
    const reply = {
      ok: false,
      error: "unknown method: changes.subscribe",
      error_code: "unknown_method",
      retryable: false,
      details: { method: "changes.subscribe" },
      unknown_future_field: 1,
    };
    const adapter = v1.create(vi.fn(async () => reply), greetingV1());
    const error = await adapter.call("changes.subscribe", {}).catch((e) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect(error.code).toBe("unknown_method");
    expect(error.details).toEqual({ method: "changes.subscribe" });
  });

  it("leaves an ApiError it is handed alone and never double-wraps", () => {
    const already = new ApiError("not_found", "no such run", { details: { id: "run-7" } });
    expect(normalizeError(already)).toBe(already);
    expect(normalizeError("plain string")).toBeInstanceOf(ApiError);
    expect(normalizeError("plain string").code).toBe("unknown");
    expect(normalizeError(undefined).message).toBe("the call failed");
  });

  it("keeps a timed-out call's own flags reachable on the ApiError", async () => {
    const timedOut = Object.assign(new Error("board.list timed out"), { timedOut: true, uncertain: true });
    const adapter = v1.create(vi.fn(async () => { throw timedOut; }), greetingV1());
    const error = await adapter.call("board.list", {}).catch((e) => e);
    expect(error.timedOut).toBe(true);
    expect(error.uncertain).toBe(true);
    expect(error.cause).toBe(timedOut);
  });

  it("parses a result, ignoring fields it does not know", () => {
    expect(v1.parseResult("git.status", { unchanged: true, from_the_future: 9 })).toEqual({
      unchanged: true,
      from_the_future: 9,
    });
    expect(() => v1.parseResult("git.status", null)).toThrow();
    expect(() => v1.parseResult("git.status", [1, 2])).toThrow();
  });

  it("parses the events it knows and no-ops on the ones it does not", () => {
    const changes = { type: "changes", subscription_id: "s-focus", items: [{ entity_id: "run-7" }] };
    expect(v1.parseEvent(changes)).toEqual(changes);
    expect(v1.parseEvent({ type: "board.changed" })).toEqual({ type: "board.changed" });
    expect(v1.parseEvent({ type: "entity.changed", id: "run-7" })).toEqual({
      type: "entity.changed",
      id: "run-7",
    });
    expect(v1.parseEvent({ type: "invented.later", payload: 1 })).toBe(null);
    expect(v1.parseEvent(null)).toBe(null);
    expect(v1.parseEvent({ id: "r1", ok: true })).toBe(null);
  });
});
