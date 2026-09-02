import { describe, it, expect } from "vitest";
import { coreSourceOf } from "./coreSource.js";
import {
  STATUS_LINE_EVENTS,
  completionReportSections,
  isStartupEvent,
  startupEventTitle,
} from "../src/core/threadEvents.js";

describe("the events a session's start is announced by", () => {
  it("names them the way the timeline used to, in the harness's own name", () => {
    expect(STATUS_LINE_EVENTS.has("session_started")).toBe(true);
    expect(STATUS_LINE_EVENTS.has("run_started")).toBe(true);
    expect(STATUS_LINE_EVENTS.has("session_ended")).toBe(false);
    expect(startupEventTitle({ event: "run_started" })).toBe("Run started");
    expect(startupEventTitle({ event: "session_started" })).toBe("Agent session started");
    expect(startupEventTitle({ event: "session_started" }, "Codex")).toBe("Codex session started");
    expect(startupEventTitle({ event: "done" })).toBe("");
    expect(startupEventTitle(null)).toBe("");
  });

  it("answers for a conversation item whether it is one of them", () => {
    expect(isStartupEvent({ type: "event", data: { event: "run_started" } })).toBe(true);
    expect(isStartupEvent({ type: "event", data: { event: "session_started" } })).toBe(true);
    expect(isStartupEvent({ type: "event", data: { event: "done" } })).toBe(false);
    expect(isStartupEvent({ type: "event" })).toBe(false);
    expect(isStartupEvent({ type: "message", data: { event: "run_started" } })).toBe(false);
  });
});

describe("where the event vocabulary sits", () => {
  it("is a pure module neither the renderer nor the rail model can be reached from", () => {
    const source = coreSourceOf("threadEvents.js");
    expect(source).not.toContain("document");
    expect(source).not.toContain('from "./thread.js"');
    expect(source).not.toContain('from "./agentRailModel.js"');
  });

  it("is where both readers take the vocabulary from, so neither imports the other", () => {
    expect(coreSourceOf("thread.js")).not.toContain('from "./agentRailModel.js"');
    expect(coreSourceOf("agentRailModel.js")).not.toContain('from "./thread.js"');
  });
});

describe("the completion report", () => {
  it("keeps the lists that were filled in, in reading order", () => {
    const sections = completionReportSections({
      critical_files: ["src/a.rs — holds the change"],
      risk_notes: [],
      decisions: ["kept the old name"],
      skips: ["did not touch the migration"],
    });
    expect(sections.map((section) => section.title)).toEqual(["Critical files", "Decisions", "Skipped"]);
    expect(sections[0].items).toEqual(["src/a.rs — holds the change"]);
  });

  it("is nothing at all when the agent filled in nothing", () => {
    expect(completionReportSections({ critical_files: [], risk_notes: [] })).toEqual([]);
    expect(completionReportSections(null)).toEqual([]);
  });
});
