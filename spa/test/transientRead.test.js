/** @vitest-environment jsdom */
// Telling a read that failed because the wire went away from a read the bridge
// refused — and what a surface does about each.
//
// The distinction is the whole of issue #24. A phone whose session dies every
// few minutes fails every call in flight, and a page that had already painted
// from the cache reported each one as an error about a copy still on screen.
// A refusal is the opposite: the bridge answered, and the reader has to hear
// it.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { createReadRetry, isTransientTransportError } from "../src/core/transientRead.js";
import { deviceBlockedMark, deviceOfflineMark } from "../src/core/text.js";

const thrown = (message, fields = {}) => Object.assign(new Error(message), fields);

describe("what counts as the wire going away", () => {
  // Each of these is a sentence one of the transport modules fails a call
  // with; `sessionSentences` below holds them to their sources.
  it("is every way a session, a carrier or a rendezvous can end", () => {
    for (const message of [
      "your device went offline",
      "your device is offline — reconnecting…",
      "session closed",
      "nothing is carrying this session",
      "the channel closed",
      "the relay socket closed",
      "the rendezvous closed",
      deviceOfflineMark,
      deviceBlockedMark,
    ]) {
      expect(isTransientTransportError(thrown(message))).toBe(true);
    }
  });

  // A deadline that expires while the call is queued behind a dead session is
  // the wire in doubt, which is what `timedOut` means (core/sessionRpc.js).
  it("is a call that ran out of time, and one whose delivery is unknown", () => {
    expect(isTransientTransportError(thrown("issues.get timed out", { timedOut: true }))).toBe(true);
    expect(isTransientTransportError(thrown("the channel went", { uncertain: true }))).toBe(true);
  });

  it("is not a refusal, whatever the bridge called it", () => {
    expect(isTransientTransportError(thrown("no such issue", { error_code: "not_found" }))).toBe(false);
    expect(isTransientTransportError(thrown("issue_id is required", { error_code: "invalid_params" }))).toBe(false);
    expect(isTransientTransportError(thrown("the bridge is busy", { error_code: "busy", retryable: true }))).toBe(false);
  });

  // A bridge speaking an API this tab cannot read IS answering. Reconnecting
  // is not the cure and waiting quietly for one would hide it forever, so it
  // is said out loud like any other refusal.
  it("is not a machine that is merely behind", () => {
    expect(isTransientTransportError(thrown("App is out of date"))).toBe(false);
    expect(isTransientTransportError(thrown("Bridge is out of date"))).toBe(false);
  });

  // Anything unrecognised is said out loud. A bug in this client that threw a
  // TypeError would otherwise be swallowed into a silent wait forever, which
  // is a worse failure than one toast too many.
  it("is not anything it does not recognise", () => {
    expect(isTransientTransportError(thrown("issues.get: result is not an object"))).toBe(false);
    expect(isTransientTransportError(new TypeError("x is not a function"))).toBe(false);
    expect(isTransientTransportError(null)).toBe(false);
    expect(isTransientTransportError(undefined)).toBe(false);
  });
});

describe("the sentences, against the modules that throw them", () => {
  // The predicate reads the transport's own words, so the words have to still
  // be there. A rewording that slipped past this would put every dropped read
  // back in a toast, which is the bug #24 was filed for.
  it("are still what those modules say", async () => {
    const { readFileSync } = await import("node:fs");
    const { resolve } = await import("node:path");
    const held = (path) => readFileSync(resolve(process.cwd(), "src/core", path), "utf8");
    const says = (path, sentence) => expect(held(path)).toContain(`"${sentence}"`);
    says("session.js", "your device went offline");
    says("session.js", "your device is offline — reconnecting…");
    says("session.js", "session closed");
    says("sessionRpc.js", "nothing is carrying this session");
    says("carrier.js", "the channel closed");
    says("carrier.js", "the relay socket closed");
    says("rendezvous.js", "the rendezvous closed");
  });
});

describe("what a surface does about one", () => {
  let host, away, reconnecting, moved, retry, hasContent, guard;

  const watch = () => ({
    away: () => away,
    reconnecting: () => reconnecting,
    moved: (fn) => {
      moved.add(fn);
      return () => moved.delete(fn);
    },
  });

  const reconnect = () => {
    away = false;
    reconnecting = false;
    [...moved].forEach((fn) => fn());
  };

  const note = () => host.querySelector(".read-wait")?.textContent ?? null;

  beforeEach(() => {
    document.body.innerHTML = '<div id="pane"><p class="painted">the cached copy</p></div>';
    host = document.querySelector("#pane");
    away = true;
    reconnecting = true;
    moved = new Set();
    retry = vi.fn();
    hasContent = () => true;
    guard = createReadRetry({
      host,
      watch: watch(),
      retry: () => retry(),
      hasContent: () => hasContent(),
      now: () => Date.parse("2026-09-20T16:42:00Z"),
    });
  });

  it("keeps a refusal out loud and waits for nothing", () => {
    expect(guard.failed(thrown("no such issue", { error_code: "not_found" }))).toBe(false);
    expect(guard.waiting()).toBe(false);
    expect(note()).toBeNull();
  });

  it("says nothing about a dropped read over a copy that is still on screen", () => {
    expect(guard.failed(thrown("your device went offline"))).toBe(true);
    expect(host.querySelector(".painted")).not.toBeNull();
  });

  it("marks when the copy was last read, while the machine is being reconnected to", () => {
    guard.succeeded(Date.parse("2026-09-20T16:40:00Z"));
    guard.failed(thrown("your device went offline"));
    expect(note()).toBe(
      `Last read at ${new Date(Date.parse("2026-09-20T16:40:00Z")).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}, reconnecting`,
    );
  });

  // A copy painted from the cache was read when the cache took it, not now.
  it("takes the time from the cached copy when no live read has landed", () => {
    guard.seen(Date.parse("2026-09-20T09:15:00Z"));
    guard.failed(thrown("the channel closed"));
    expect(note()).toContain("Last read at");
  });

  // The machine is simply gone: the strip over the surface names it
  // (core/deviceNotice.js), and a second line claiming something is happening
  // would be a claim nothing is backing.
  it("marks nothing while nothing is being done about the machine", () => {
    reconnecting = false;
    guard.failed(thrown("your device went offline"));
    expect(note()).toBeNull();
  });

  it("reads again once the machine is back, and clears the mark", () => {
    guard.failed(thrown("your device went offline"));
    expect(retry).not.toHaveBeenCalled();
    reconnect();
    expect(retry).toHaveBeenCalledTimes(1);
    guard.succeeded();
    expect(note()).toBeNull();
    expect(guard.waiting()).toBe(false);
  });

  // The registry and the supervisor announce one reconnect twice.
  it("reads once however many times the machine says it is back", () => {
    guard.failed(thrown("your device went offline"));
    away = false;
    reconnecting = false;
    // One `moved` listener, told twice — which is what the registry and the
    // supervisor between them do on one reconnect.
    const [told] = [...moved];
    told();
    told();
    expect(retry).toHaveBeenCalledTimes(1);
  });

  it("waits again when the machine goes a second time", () => {
    guard.failed(thrown("your device went offline"));
    reconnect();
    away = true;
    guard.failed(thrown("your device went offline"));
    reconnect();
    expect(retry).toHaveBeenCalledTimes(2);
  });

  // The point of the quiet: with nothing on screen there is nothing to be
  // quiet about, so the retry's own failure is said out loud.
  it("says the retry's failure out loud when there is no copy on screen", () => {
    hasContent = () => false;
    expect(guard.failed(thrown("your device went offline"))).toBe(true);
    reconnect();
    expect(guard.failed(thrown("your device went offline"))).toBe(false);
  });

  it("stays quiet through the retry when there is a copy on screen", () => {
    guard.failed(thrown("your device went offline"));
    reconnect();
    away = true;
    expect(guard.failed(thrown("your device went offline"))).toBe(true);
  });

  // The read knows first. Measured on the compose stack: with the bridge
  // paused the call timed out at +12s and the ring did not say Reconnecting
  // until +15s — so a guard that asks "is the machine away?" as the read fails
  // is told "no" about a session that is already dead. Asking then, and
  // believing the answer, is why the mark never appeared and the retry never
  // armed: the whole of #24, silently absent on a live stack.
  it("waits even while the registry still thinks the machine is here", () => {
    away = false;
    reconnecting = false;
    expect(guard.failed(thrown("issues.get timed out", { timedOut: true }))).toBe(true);
    expect(guard.waiting()).toBe(true);
    expect(moved.size).toBe(1);
  });

  it("marks as soon as the registry catches up, without another failure", () => {
    away = false;
    reconnecting = false;
    guard.succeeded(Date.parse("2026-09-20T16:40:00Z"));
    guard.failed(thrown("issues.get timed out", { timedOut: true }));
    expect(note()).toBeNull();
    away = true;
    reconnecting = true;
    [...moved].forEach((fn) => fn());
    expect(note()).toContain("Last read at");
  });

  it("reads again on the machine coming back, having gone after the read died", () => {
    away = false;
    reconnecting = false;
    guard.failed(thrown("issues.get timed out", { timedOut: true }));
    away = true;
    [...moved].forEach((fn) => fn());
    expect(retry).not.toHaveBeenCalled();
    reconnect();
    expect(retry).toHaveBeenCalledTimes(1);
  });

  // Coming back from never having gone is not a reconnect. Reading on that
  // would be a read per announcement, for as long as the surface is mounted.
  it("reads nothing on an announcement about a machine that never went", () => {
    away = false;
    reconnecting = false;
    guard.failed(thrown("issues.get timed out", { timedOut: true }));
    [...moved].forEach((fn) => fn());
    [...moved].forEach((fn) => fn());
    expect(retry).not.toHaveBeenCalled();
  });

  // Quiet is for a copy on screen. An empty surface that the machine has no
  // explanation for would otherwise sit on its loading frame saying nothing.
  it("says one out loud when there is no copy and the machine claims to be here", () => {
    away = false;
    reconnecting = false;
    hasContent = () => false;
    expect(guard.failed(thrown("issues.get timed out", { timedOut: true }))).toBe(false);
    expect(guard.waiting()).toBe(true);
  });

  it("lets go of its subscription when the surface does", () => {
    guard.failed(thrown("your device went offline"));
    expect(moved.size).toBe(1);
    guard.dispose();
    expect(moved.size).toBe(0);
  });
});
