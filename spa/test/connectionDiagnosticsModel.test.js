import { describe, it, expect } from "vitest";
import {
  deviceIdOfDiagnostic,
  diagnosticDetailText,
  diagnosticDeviceLabel,
  diagnosticRows,
  diagnosticsJson,
  diagnosticsSummary,
  formatDiagnosticTime,
  shortDeviceId,
} from "../src/core/connectionDiagnosticsModel.js";

const MAC = "3f2a91c4-0b1d-4e77-9a21-6c5b8e0d4417";
const PHONE = "8c17d2e5-77aa-4b90-8f31-2d9e4c6a1b03";
const DEVICES = [
  { id: MAC, name: "Mac mini", status: "online" },
  { id: PHONE, name: "Pixel", status: "offline" },
];

const entry = (over = {}) => ({ at: Date.parse("2026-09-20T10:15:04Z"), connection: `${MAC}:sess-1`, event: "state", ...over });

describe("the machine a diagnostic is about", () => {
  it("is the half of the id before the first colon", () => {
    expect(deviceIdOfDiagnostic(`${MAC}:sess-1`)).toBe(MAC);
  });

  it("is nobody for an id in any other shape", () => {
    expect(deviceIdOfDiagnostic("no-colon-here")).toBe("");
    expect(deviceIdOfDiagnostic(":leading")).toBe("");
    expect(deviceIdOfDiagnostic(undefined)).toBe("");
    expect(deviceIdOfDiagnostic(42)).toBe("");
  });

  it("is named by the account list where the list knows it", () => {
    expect(diagnosticDeviceLabel(`${MAC}:sess-1`, DEVICES)).toBe("Mac mini");
    expect(diagnosticDeviceLabel(`${PHONE}:sess-9`, DEVICES)).toBe("Pixel");
  });

  it("falls back to the short id for a machine the list has never heard of", () => {
    expect(diagnosticDeviceLabel("ffffffff-dead-4000-8000-000000000000:s", DEVICES)).toBe("ffffffff…");
    expect(shortDeviceId("ffffffff-dead")).toBe("ffffffff…");
    expect(shortDeviceId("")).toBe("unknown device");
  });

  it("says so plainly when the id names nothing at all", () => {
    expect(diagnosticDeviceLabel("relay-only", DEVICES)).toBe("unknown device");
    expect(diagnosticDeviceLabel(undefined, [])).toBe("unknown device");
  });

  it("names the local cache's events as this browser's (#169)", () => {
    expect(diagnosticDeviceLabel("local-cache", DEVICES)).toBe("this browser");
  });
});

describe("the time on a row", () => {
  it("carries seconds, because these arrive in bursts", () => {
    expect(formatDiagnosticTime(Date.parse("2026-09-20T10:15:04Z"))).toMatch(/^\d{2}:\d{2}:\d{2}$/);
  });

  it("says nothing rather than Invalid Date for a stamp that is not one", () => {
    expect(formatDiagnosticTime("whenever")).toBe("—");
    expect(formatDiagnosticTime(undefined)).toBe("—");
  });
});

describe("the detail on a row", () => {
  it("is every field past the envelope, as key=value", () => {
    expect(diagnosticDetailText(entry({ state: "failed", phase: "restart" }))).toBe("state=failed phase=restart");
  });

  it("leaves the envelope out of it", () => {
    expect(diagnosticDetailText(entry())).toBe("");
  });

  it("keeps the order the event recorded", () => {
    expect(diagnosticDetailText(entry({ channel: "app", state: "closed" }))).toBe("channel=app state=closed");
  });

  it("writes a nested value as json and an absent one as nothing", () => {
    expect(diagnosticDetailText(entry({ pair: { local: "host" }, reason: null }))).toBe('pair={"local":"host"} reason=');
  });

  it("survives an entry that is not an object", () => {
    expect(diagnosticDetailText(null)).toBe("");
  });
});

describe("the history as rows", () => {
  const history = [
    entry({ at: 1000, event: "negotiating", phase: "initial" }),
    entry({ at: 2000, event: "connected", phase: "initial" }),
    entry({ at: 3000, connection: `${PHONE}:sess-9`, event: "restart-failed", reason: "timeout" }),
  ];

  it("puts the newest first", () => {
    expect(diagnosticRows(history, DEVICES).map((row) => row.kind)).toEqual([
      "restart-failed",
      "connected",
      "negotiating",
    ]);
  });

  it("names the machine, the connection, the kind and the detail on each", () => {
    const [newest] = diagnosticRows(history, DEVICES);
    expect(newest.device).toBe("Pixel");
    expect(newest.source).toBe(`${PHONE}:sess-9`);
    expect(newest.kind).toBe("restart-failed");
    expect(newest.detail).toBe("reason=timeout");
    expect(newest.at).toBe(3000);
  });

  it("keeps recording order rather than re-sorting on a clock that stepped", () => {
    const stepped = [entry({ at: 9000, event: "first" }), entry({ at: 1000, event: "second" })];
    expect(diagnosticRows(stepped, DEVICES).map((row) => row.kind)).toEqual(["second", "first"]);
  });

  it("gives two events recorded in the same millisecond different keys", () => {
    const together = [entry({ at: 5000, event: "a" }), entry({ at: 5000, event: "b" })];
    const [first, second] = diagnosticRows(together, DEVICES);
    expect(first.key).not.toBe(second.key);
  });

  it("falls back to the channel when an entry carries no connection", () => {
    expect(diagnosticRows([{ at: 1, event: "channel", channel: "app" }], DEVICES)[0].source).toBe("app");
  });

  it("survives an empty history and a missing one", () => {
    expect(diagnosticRows([], DEVICES)).toEqual([]);
    expect(diagnosticRows(null, DEVICES)).toEqual([]);
    expect(diagnosticRows(history)).toHaveLength(3);
  });
});

describe("what gets copied and shared", () => {
  it("is the raw events, so support reads what OPS.md describes", () => {
    const events = [entry({ at: 1000, event: "state", state: "failed" })];
    const copied = JSON.parse(diagnosticsJson({ since: 900, dropped: 0, events }));

    expect(copied.events).toEqual(events);
    expect(diagnosticsJson({ events })).toContain("\n  ");
  });

  // #60: the report that prompted it carried only the final session, because the
  // failures had been pushed out of the ring by the storm they caused and nothing
  // in the paste said so. A dump that cannot admit it is incomplete reads as a
  // complete account of a connection that simply worked.
  it("says when the tab started recording and how many events it had to drop", () => {
    const copied = JSON.parse(diagnosticsJson({ since: 1000, dropped: 42, events: [] }));

    expect(copied).toEqual({ since: 1000, dropped: 42, events: [] });
  });

  it("takes the events alone from a caller that has only those, claiming nothing about drops", () => {
    const events = [entry({ at: 1000, event: "state" })];

    expect(JSON.parse(diagnosticsJson(events))).toEqual({ since: null, dropped: 0, events });
  });

  it("is an empty record rather than nothing when nothing was recorded", () => {
    expect(JSON.parse(diagnosticsJson([]))).toEqual({ since: null, dropped: 0, events: [] });
    expect(JSON.parse(diagnosticsJson(null))).toEqual({ since: null, dropped: 0, events: [] });
  });
});

describe("the line above the list", () => {
  it("counts the events, and says nothing when there are none", () => {
    expect(diagnosticsSummary([{}, {}])).toBe("2 recorded events, newest first.");
    expect(diagnosticsSummary([{}])).toBe("1 recorded event, newest first.");
    expect(diagnosticsSummary([])).toBe("");
  });
});
