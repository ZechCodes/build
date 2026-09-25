// @vitest-environment jsdom
// Settings → Diagnostics, mounted.
//
// The panel exists because the reconnects happen on a phone, where there is no
// console to run `buildConnectionDiagnostics()` in. So what it has to do is:
// show the events newest first, keep showing them as more arrive, and get the
// dump off the device — by clipboard where there is one, by share sheet where
// there is only a thumb.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  connectionDiagnosticsPanelHtml,
  mountConnectionDiagnostics,
} from "../src/core/connectionDiagnosticsPanel.js";

const MAC = "3f2a91c4-0b1d-4e77-9a21-6c5b8e0d4417";
const PHONE = "8c17d2e5-77aa-4b90-8f31-2d9e4c6a1b03";
const DEVICES = [{ id: MAC, name: "Mac mini", status: "online" }];

const entry = (over = {}) => ({ at: 1000, connection: `${MAC}:sess-1`, event: "state", ...over });

let recorded;
let dispose;

const host = () => document.querySelector("#host");
const rows = () => [...document.querySelectorAll(".diagrow")];
const rowText = (row) => row.textContent.replace(/\s+/g, " ").trim();
const summary = () => document.querySelector("#diagsummary").textContent;
const error = () => document.querySelector("#diagerr").textContent;
const button = (id) => document.querySelector(`#${id}`);

const mount = (over = {}) => {
  dispose = mountConnectionDiagnostics(host(), {
    history: () => recorded,
    // What the copy and the share carry: the whole record, not just the events.
    report: () => ({ since: 1000, dropped: 0, events: recorded }),
    clear: () => { recorded = []; },
    devices: () => DEVICES,
    clipboard: null,
    share: null,
    ...over,
  });
  return dispose;
};

beforeEach(() => {
  vi.useFakeTimers();
  recorded = [];
  document.body.innerHTML = `<div id="host">${connectionDiagnosticsPanelHtml()}</div>`;
});

afterEach(() => {
  dispose?.();
  dispose = null;
  vi.useRealTimers();
  document.body.innerHTML = "";
});

describe("the events on the page", () => {
  it("lists them newest first, with the machine named", () => {
    recorded = [
      entry({ at: 1000, event: "negotiating", phase: "initial" }),
      entry({ at: 2000, connection: `${PHONE}:sess-9`, event: "restart-failed", reason: "timeout" }),
    ];

    mount();

    expect(rows()).toHaveLength(2);
    expect(rowText(rows()[0])).toContain("restart-failed");
    expect(rowText(rows()[1])).toContain("negotiating");
    expect(rowText(rows()[1])).toContain("Mac mini");
  });

  it("shows the short id for a machine the account list has never heard of", () => {
    recorded = [entry({ connection: `${PHONE}:sess-9` })];

    mount();

    expect(rowText(rows()[0])).toContain("8c17d2e5…");
  });

  it("puts the connection and the detail on their own monospace lines", () => {
    recorded = [entry({ event: "channel", channel: "app", state: "closed" })];

    mount();

    expect(document.querySelector(".diagrow-source").textContent).toBe(`${MAC}:sess-1`);
    expect(document.querySelector(".diagrow-source").classList.contains("mono")).toBe(true);
    expect(document.querySelector(".diagrow-detail").textContent).toBe("channel=app state=closed");
    expect(document.querySelector(".diagrow-detail").classList.contains("mono")).toBe(true);
  });

  it("counts them above the list", () => {
    recorded = [entry(), entry({ at: 2000 })];

    mount();

    expect(summary()).toBe("2 recorded events, newest first.");
  });

  it("says nothing is recorded rather than showing an empty box", () => {
    mount();

    expect(rows()).toHaveLength(0);
    expect(document.querySelector("#diaglist").textContent).toContain("Nothing recorded yet");
    expect(summary()).toBe("");
  });

  it("says on the page that the dump holds no content or keys", () => {
    mount();

    const note = document.querySelector("#diagnostics").textContent;
    expect(note).toContain("Timestamps, connection identifiers and the browser's storage errors only");
    expect(note).toContain("no message content, no keys");
  });
});

describe("while the section is open", () => {
  it("picks up events recorded after the mount", () => {
    mount();
    expect(rows()).toHaveLength(0);

    recorded = [entry({ event: "connected" })];
    vi.advanceTimersByTime(1000);

    expect(rows()).toHaveLength(1);
    expect(rowText(rows()[0])).toContain("connected");
    expect(summary()).toBe("1 recorded event, newest first.");
  });

  it("leaves the list alone on a tick that would say the same thing", () => {
    recorded = [entry()];
    mount();
    const standing = rows()[0];

    vi.advanceTimersByTime(3000);

    expect(rows()[0]).toBe(standing);
  });

  it("stops reading the history once disposed", () => {
    mount();
    dispose();
    dispose = null;

    recorded = [entry({ event: "connected" })];
    vi.advanceTimersByTime(5000);

    expect(rows()).toHaveLength(0);
  });
});

describe("getting the dump off the device", () => {
  it("puts the whole record on the clipboard as json, drops included", async () => {
    recorded = [entry({ event: "state", state: "failed" })];
    const writeText = vi.fn(async () => {});
    mount({ clipboard: { writeText }, report: () => ({ since: 1000, dropped: 7, events: recorded }) });

    button("diagcopy").click();
    await vi.waitFor(() => expect(button("diagcopy").textContent).toBe("Copied"));

    // The dropped count is the point: a paste that cannot say it is incomplete is
    // what sent #60 looking in the bridge log instead of the reader's own report.
    expect(JSON.parse(writeText.mock.calls[0][0])).toEqual({
      since: 1000,
      dropped: 7,
      events: recorded,
    });
  });

  it("falls back to a textarea when the browser offers no clipboard", async () => {
    recorded = [entry()];
    document.execCommand = vi.fn(() => true);
    mount({ clipboard: null });

    button("diagcopy").click();
    await vi.waitFor(() => expect(document.execCommand).toHaveBeenCalledWith("copy"));

    expect(error()).toBe("");
    // The holder is taken away again: nothing of it is left on the page.
    expect(document.querySelector("textarea")).toBe(null);
    delete document.execCommand;
  });

  it("says so when neither route works, rather than claiming it copied", async () => {
    recorded = [entry()];
    document.execCommand = vi.fn(() => false);
    mount({ clipboard: { writeText: async () => { throw new Error("denied"); } } });

    button("diagcopy").click();
    await vi.waitFor(() => expect(error()).toContain("would not take the clipboard"));

    expect(button("diagcopy").textContent).toBe("Copy");
    delete document.execCommand;
  });

  it("hides Share on a browser that has no share sheet", () => {
    mount({ share: null });

    expect(button("diagshare").hidden).toBe(true);
  });

  it("shows Share where there is one, and hands it the json", async () => {
    recorded = [entry({ event: "restarting" })];
    const share = vi.fn(async () => {});
    mount({ share });

    expect(button("diagshare").hidden).toBe(false);
    button("diagshare").click();
    await vi.waitFor(() => expect(share).toHaveBeenCalled());

    expect(JSON.parse(share.mock.calls[0][0].text)).toEqual({
      since: 1000,
      dropped: 0,
      events: recorded,
    });
  });

  it("stays quiet when the share sheet is dismissed", async () => {
    recorded = [entry()];
    const refusal = Object.assign(new Error("share cancelled"), { name: "AbortError" });
    const share = vi.fn(async () => { throw refusal; });
    mount({ share });

    button("diagshare").click();
    await vi.waitFor(() => expect(share).toHaveBeenCalled());

    expect(error()).toBe("");
  });

  it("names a share that actually failed", async () => {
    recorded = [entry()];
    const share = vi.fn(async () => { throw new Error("no target"); });
    mount({ share });

    button("diagshare").click();
    await vi.waitFor(() => expect(error()).toBe("no target"));
  });
});

describe("clearing", () => {
  it("empties the history and the list with it", () => {
    recorded = [entry(), entry({ at: 2000 })];
    mount();
    expect(rows()).toHaveLength(2);

    button("diagclear").click();

    expect(recorded).toEqual([]);
    expect(rows()).toHaveLength(0);
    expect(document.querySelector("#diaglist").textContent).toContain("Nothing recorded yet");
    expect(button("diagclear").textContent).toBe("Cleared");
  });
});

describe("a host with no panel in it", () => {
  it("mounts nothing and hands back a dispose that is safe to call", () => {
    document.body.innerHTML = '<div id="host"></div>';

    const teardown = mountConnectionDiagnostics(host(), { history: () => [] });

    expect(() => teardown()).not.toThrow();
  });
});
