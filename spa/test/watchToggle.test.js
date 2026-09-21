/** @vitest-environment jsdom */
// Watching an issue, from whichever surface offers the switch.
//
// #65. The issue page's toggle and the inbox's Mute are the same verb, so the
// optimistic rule lives once: the switch moves under the finger, and a refusal
// puts it back rather than leaving the reader believing something the bridge
// never agreed to.

import { describe, it, expect, vi } from "vitest";

import { createWatchToggle, syncWatchButton, watchButtonHtml, watchTitle } from "../src/core/watchToggle.js";

const settled = () => new Promise((done) => setTimeout(done, 0));

describe("what the switch says", () => {
  it("names who else is watching, and says so in one line", () => {
    expect(watchTitle({ watching: true, watchers: 3 })).toBe("Watching · 3");
    expect(watchTitle({ watching: true, watchers: 1 })).toBe("Watching · 1");
    // Nobody else is not a number worth showing.
    expect(watchTitle({ watching: true, watchers: 0 })).toBe("Watching");
    expect(watchTitle({ watching: false, watchers: 3 })).toBe("Not watching · 3");
    expect(watchTitle({ watching: false, watchers: 0 })).toBe("Not watching");
  });
});

describe("the switch moving", () => {
  it("moves before the bridge answers, and counts the reader in", async () => {
    const call = vi.fn(async () => ({}));
    const seen = [];
    const toggle = createWatchToggle({ watching: false, watchers: 2, issueId: "i-1", call, onChange: (s) => seen.push({ ...s }) });

    const settledPromise = toggle.press();
    // Optimistic: the reader sees it on before the call resolves.
    expect(toggle.state()).toEqual({ watching: true, watchers: 3, pending: true });
    expect(seen[0]).toMatchObject({ watching: true, watchers: 3 });

    await settledPromise;
    expect(call).toHaveBeenCalledWith("issues.watch", { issue_id: "i-1" });
    expect(toggle.state()).toEqual({ watching: true, watchers: 3, pending: false });
  });

  it("asks to unwatch when it was on, and counts the reader out", async () => {
    const call = vi.fn(async () => ({}));
    const toggle = createWatchToggle({ watching: true, watchers: 3, issueId: "i-1", call });
    await toggle.press();
    expect(call).toHaveBeenCalledWith("issues.unwatch", { issue_id: "i-1" });
    expect(toggle.state()).toMatchObject({ watching: false, watchers: 2 });
  });

  it("puts the switch back when the bridge refuses", async () => {
    const call = vi.fn(async () => {
      throw new Error("no such issue");
    });
    const failures = [];
    const toggle = createWatchToggle({ watching: false, watchers: 2, issueId: "i-1", call, onFailure: (e) => failures.push(e) });

    await toggle.press();

    expect(toggle.state()).toEqual({ watching: false, watchers: 2, pending: false });
    expect(failures.map((error) => error.message)).toEqual(["no such issue"]);
  });

  // A reader pressing twice while the first is in flight would otherwise send
  // watch, then unwatch, and land on whichever answered last.
  it("ignores a second press while one is in flight", async () => {
    let release;
    const call = vi.fn(() => new Promise((resolve) => { release = resolve; }));
    const toggle = createWatchToggle({ watching: false, watchers: 0, issueId: "i-1", call });

    const first = toggle.press();
    toggle.press();
    expect(call).toHaveBeenCalledTimes(1);

    release({});
    await first;
    expect(toggle.state()).toMatchObject({ watching: true, pending: false });
  });

  // The push is the authority; a row arriving says what is true regardless of
  // what this switch last guessed.
  it("takes what the bridge says, and drops a guess that disagrees", () => {
    const toggle = createWatchToggle({ watching: false, watchers: 0, issueId: "i-1", call: async () => ({}) });
    toggle.settle({ watching: true, watchers: 4 });
    expect(toggle.state()).toEqual({ watching: true, watchers: 4, pending: false });
  });
});

describe("what it is a watch of", () => {
  // Two kinds of thing are watched and the wire names them differently (#64).
  // The switch behaves the same for both; only the verb and its parameter move.
  it("asks about a conversation by its own id", async () => {
    const call = vi.fn(async () => ({}));
    const toggle = createWatchToggle({ watching: false, watchers: 0, conversationId: "run-7", call });
    await toggle.press();
    expect(call).toHaveBeenCalledWith("conversation.watch", { conversation_id: "run-7" });
    await toggle.press();
    expect(call).toHaveBeenLastCalledWith("conversation.unwatch", { conversation_id: "run-7" });
  });

  it("stays on the issue verbs when it is an issue, which is what the issue page imports", async () => {
    const call = vi.fn(async () => ({}));
    await createWatchToggle({ watching: false, watchers: 0, issueId: "i-1", call }).press();
    expect(call).toHaveBeenCalledWith("issues.watch", { issue_id: "i-1" });
  });
});

describe("the control it wears", () => {
  const button = (html) => {
    document.body.innerHTML = html;
    return document.body.firstElementChild;
  };

  it("says whether it is on, for a reader who cannot see the icon", () => {
    expect(button(watchButtonHtml({ watching: true, watchers: 3 })).getAttribute("aria-pressed")).toBe("true");
    expect(button(watchButtonHtml({ watching: false, watchers: 0 })).getAttribute("aria-pressed")).toBe("false");
  });

  it("carries the count where hovering finds it", () => {
    const on = button(watchButtonHtml({ watching: true, watchers: 3 }));
    expect(on.getAttribute("title")).toBe("Watching · 3");
    expect(on.getAttribute("aria-label")).toBe("Watching · 3");
  });

  it("is written onto a button already standing, so a press rebuilds no head", () => {
    const node = button(watchButtonHtml({ watching: false, watchers: 2 }));
    syncWatchButton(node, { watching: true, watchers: 3, pending: false });
    expect(node.getAttribute("aria-pressed")).toBe("true");
    expect(node.getAttribute("title")).toBe("Watching · 3");
    expect(node.classList.contains("watching")).toBe(true);
  });

  it("is not pressable while a verb is in flight", () => {
    const node = button(watchButtonHtml({ watching: false, watchers: 0 }));
    syncWatchButton(node, { watching: true, watchers: 1, pending: true });
    expect(node.disabled).toBe(true);
    syncWatchButton(node, { watching: true, watchers: 1, pending: false });
    expect(node.disabled).toBe(false);
  });
});
