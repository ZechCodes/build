// @vitest-environment jsdom
// What the chat paint costs, and what it is allowed to skip.
//
// The conversation repaints on a 1.6s poll and on every event. Two things keep
// that cheap: a fingerprint of everything the paint is drawn from, so a tick
// that resolved the same conversation builds nothing at all, and a keyed list,
// so the rows that did move are the only ones the document is asked to change.

import { describe, expect, it } from "vitest";
import {
  chatPaintFingerprint,
  paintThreadEntries,
  threadOfferState,
  timelineEntries,
  wireThreadOptions,
} from "../src/core/thread.js";

const INPUTS = {
  deliveredSequence: 40,
  itemCount: 12,
  digests: [{ from_sequence: 1, through_sequence: 40, tool_calls: 300, rows: 320, last_tool_call: { sequence: 39, outcome: "ok" } }],
  openRunKeys: new Set(["1"]),
  fetchedRunKeys: new Set(),
  selectedAgentId: "ag-1",
  agentLabel: "Claude Code",
  sending: "",
  choiceState: "",
};

const withInput = (over) => chatPaintFingerprint({ ...INPUTS, ...over });

describe("the chat paint's fingerprint", () => {
  it("says the same thing for the same inputs", () => {
    expect(withInput({})).toBe(withInput({}));
    expect(withInput({ openRunKeys: new Set(["1"]) })).toBe(withInput({}));
  });

  it("moves when any one of the inputs the paint reads moves", () => {
    const moved = {
      deliveredSequence: 41,
      itemCount: 13,
      digests: [{ from_sequence: 1, through_sequence: 40, tool_calls: 300, rows: 321, last_tool_call: { sequence: 39, outcome: "ok" } }],
      openRunKeys: new Set(),
      fetchedRunKeys: new Set(["1"]),
      selectedAgentId: "ag-2",
      agentLabel: "Codex",
      sending: "th-1::m-4",
      choiceState: "th-1::m-4=go",
    };

    for (const [name, value] of Object.entries(moved)) {
      expect(withInput({ [name]: value }), `${name} left the fingerprint standing`).not.toBe(withInput({}));
    }
  });

  it("reads the same set whatever order the keys arrived in", () => {
    expect(withInput({ openRunKeys: new Set(["1", "9"]) })).toBe(withInput({ openRunKeys: new Set(["9", "1"]) }));
  });
});

describe("the offer state a paint has to see move", () => {
  const offerThread = () => {
    document.body.innerHTML = `<div id="host"></div>`;
    const host = document.getElementById("host");
    const items = [{
      type: "message",
      data: { sequence: 3, id: "m-4", role: "agent", body: "Which way?", options: [{ id: "go", label: "Go" }] },
    }];
    paintThreadEntries(host, timelineEntries(items, "Claude Code", "th-1", []));
    return host;
  };

  it("moves when a chip is picked, and again when the pick is sent", () => {
    const host = offerThread();
    let held = null;
    wireThreadOptions(host, () => new Promise((resolve) => {
      held = resolve;
    }));
    const before = threadOfferState();

    host.querySelector(".thread-option").click();
    const picked = threadOfferState();
    expect(picked).not.toEqual(before);

    host.querySelector(".thread-options-send").click();
    expect(threadOfferState()).not.toEqual(picked);
    held({});
  });
});

describe("painting the timeline as keyed rows", () => {
  const message = (sequence, body) => ({ type: "message", data: { sequence, role: "agent", body } });
  const host = () => document.getElementById("host");
  const paint = (items, options) => {
    if (!document.getElementById("host")) document.body.innerHTML = `<div id="host"></div>`;
    return paintThreadEntries(host(), timelineEntries(items, "Claude Code", "th-1", [], options), options || {});
  };
  const rows = () => [...host().querySelector(".thread-items").children];

  it("writes the conversation's frame once, and only once", () => {
    document.body.innerHTML = `<div id="host"></div>`;

    expect(paint([message(1, "one")])).toBe(true);
    const timeline = host().querySelector(".thread-items");

    expect(paint([message(1, "one"), message(2, "two")])).toBe(false);
    expect(host().querySelector(".thread-items")).toBe(timeline);
  });

  it("keeps the element of a row nothing moved under", () => {
    document.body.innerHTML = `<div id="host"></div>`;
    paint([message(1, "one"), message(2, "two")]);
    const [first, second] = rows();

    paint([message(1, "one"), message(2, "two"), message(3, "three")]);

    expect(rows()).toHaveLength(3);
    expect(rows()[0]).toBe(first);
    expect(rows()[1]).toBe(second);
  });

  it("keys each row by the sequence it stands at", () => {
    document.body.innerHTML = `<div id="host"></div>`;
    paint([message(1, "one"), message(2, "two")]);

    expect(rows().map((row) => row.dataset.key)).toEqual(["1", "2"]);
  });

  it("says a conversation with nothing on it is empty, and stops saying so", () => {
    document.body.innerHTML = `<div id="host"></div>`;
    paint([]);

    expect(host().querySelectorAll(".thread-empty")).toHaveLength(1);
    expect(host().querySelector(".review-thread").classList.contains("is-empty")).toBe(true);

    paint([message(1, "one")]);

    expect(host().querySelectorAll(".thread-empty")).toHaveLength(0);
    expect(host().querySelector(".review-thread").classList.contains("is-empty")).toBe(false);
    expect(host().querySelector(".thread-title-text").textContent).toContain("1");
  });

  it("leaves a fold the reader opened open across a repaint", () => {
    document.body.innerHTML = `<div id="host"></div>`;
    const items = [{ type: "event", data: { sequence: 5, event: "tool_use", summary: "Read a.js\nlines 1-40" } }];
    paint(items, { openRuns: new Set(["5"]) });
    const fold = host().querySelector(".thread-activity");
    fold.open = true;

    paint([...items, { type: "event", data: { sequence: 6, event: "reasoning", summary: "Thinking." } }], {
      openRuns: new Set(["5"]),
    });

    expect(host().querySelector(".thread-activity")).toBe(fold);
    expect(fold.open).toBe(true);
  });
});
