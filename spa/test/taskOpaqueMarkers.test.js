import { Buffer } from "node:buffer";
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  compareTaskMarkers, effectiveTaskWatchFloor, isPresentTaskMarker,
  newerTaskMarker, taskMarkerAfter, taskMarkerCovers, taskMarkerSuffix,
} from "../src/core/taskOpaqueMarkers.js";
import { latestTaskMark, taskUnreadKey, taskUnreadReading, taskWatchBaseline } from "../src/core/trackerUnread.js";
import { unreadAgentComments } from "../src/core/trackerAttentionModel.js";

// Independent source oracle: Rust str orders valid UTF-8 bytes, and when()
// removes everything through the FIRST hyphen, preserving the whole remainder.
const rustWhen = (id) => id.replace(/^[^-]*-/u, "");
const rustCompare = (left, right) => Math.sign(Buffer.compare(Buffer.from(rustWhen(left)), Buffer.from(rustWhen(right))));
const rustAfter = (mark, id) => mark == null || rustCompare(id, mark) > 0;
const watched = (id) => ({ type: "event", kind: "watched", id });
const rustWatch = (task, timeline) => {
  if (task.watch_started_after != null) return task.watch_started_after;
  let floor = null;
  for (const entry of timeline) {
    if (entry.type === "event" && entry.kind === "watched" &&
      (floor === null || rustCompare(entry.id, floor) >= 0)) floor = entry.id;
  }
  return floor;
};

// Literal expectations characterize the CURRENT SPA oracle separately. They
// deliberately include behavior that disagrees with the canonical Rust source.
const extraction = [
  ["tc-A", "A", "A"], ["te-A", "A", "A"], ["ic-A", "A", "A"],
  ["unknown-A", "A", null], ["nohyphen", "nohyphen", null],
  ["", "", null], ["te-", "", null], ["-", "", null],
  ["tc-A-B-C", "A-B-C", null], ["--tail", "-tail", null],
  ["tc-\0", "\0", "\0"], ["te- \t\n", " \t\n", " \t\n"],
  ["prefix-尾-\0 ", "尾-\0 ", null], ["tc-é", "é", "é"],
  ["tc-e\u0301", "e\u0301", "e\u0301"], ["tc-\ue000", "\ue000", "\ue000"],
  ["te-\u{10000}", "\u{10000}", "\u{10000}"],
  [`tc-${"a".repeat(125)}`, "a".repeat(125), "a".repeat(125)],
];

describe("canonical opaque task marker source oracle", () => {
  it("pins the exact bridge source semantics being modeled", () => {
    const summary = readFileSync(new URL("../../bridge/src/tracker/summary.rs", import.meta.url), "utf8");
    const tracking = readFileSync(new URL("../../bridge/src/app/tracker/tracking.rs", import.meta.url), "utf8");
    expect(summary).toContain("mark.is_none_or(|mark| when(id) > when(mark))");
    expect(summary).toContain("id.split_once('-').map_or(id, |(_, ulid)| ulid)");
    expect(summary).toContain("task.watch_started_after.as_deref().or_else(||");
    expect(summary).toContain(".max_by_key(|id| when(id))");
    expect(tracking).toContain("super::inbox::when(read) >= super::inbox::when(event_id.as_str())");
  });

  it.each(extraction)("preserves the complete canonical suffix of %j", (id, suffix) => {
    expect(Buffer.byteLength(id)).toBeLessThanOrEqual(128);
    expect(taskMarkerSuffix(id)).toBe(suffix);
    expect(taskMarkerSuffix(id)).toBe(rustWhen(id));
  });

  it("distinguishes absence from every string, including an empty marker", () => {
    for (const absent of [null, undefined]) {
      expect(isPresentTaskMarker(absent)).toBe(false);
      expect(compareTaskMarkers(absent, "")).toBe(-1);
      expect(taskMarkerCovers(absent, "")).toBe(false);
      expect(taskMarkerAfter(absent, "")).toBe(true);
      expect(newerTaskMarker(absent, "")).toBe("");
    }
    for (const marker of ["", "\0", " \t", "te-"]) expect(isPresentTaskMarker(marker)).toBe(true);
    expect(compareTaskMarkers(null, undefined)).toBe(0);
    expect(newerTaskMarker(undefined, null)).toBeNull();
    expect(taskMarkerAfter("", "te-")).toBe(false);
    expect(taskMarkerCovers("", "te-")).toBe(true);
  });

  it("matches Rust UTF-8 order, strict after, and inclusive read coverage across the corpus", () => {
    for (const [left] of extraction) {
      for (const [right] of extraction) {
        const comparison = rustCompare(left, right);
        expect(compareTaskMarkers(left, right), JSON.stringify([left, right])).toBe(comparison);
        expect(taskMarkerCovers(left, right)).toBe(comparison >= 0);
        expect(taskMarkerAfter(left, right)).toBe(rustAfter(left, right));
        expect(newerTaskMarker(left, right)).toBe(comparison < 0 ? right : left);
      }
    }
  });

  it("orders supplementary scalars after BMP scalars without Unicode normalization", () => {
    expect("\u{10000}" < "\ue000").toBe(true); // JS UTF-16 reverses Rust's order here.
    expect(compareTaskMarkers("tc-\u{10000}", "te-\ue000")).toBe(1);
    expect(compareTaskMarkers("tc-é", "te-e\u0301")).toBe(1);
    expect(taskMarkerCovers("tc-A-B", "te-A")).toBe(true);
    expect(taskMarkerCovers("tc-A", "te-A-B")).toBe(false);
  });

  it("rejects invalid JS-only strings rather than replacing lone surrogates with U+FFFD", () => {
    for (const marker of ["tc-\ud800", "tc-\udfff"]) {
      expect(() => taskMarkerSuffix(marker)).toThrow(TypeError);
      expect(() => compareTaskMarkers(marker, "tc-�")).toThrow(TypeError);
    }
  });

  const timeline = [
    watched("te-A"), { type: "comment", kind: "watched", id: "tc-Z" },
    { type: "event", kind: "moved", id: "te-Z" }, watched("unknown-A-Z"),
  ];
  it.each([undefined, null, "", "te-", "nohyphen", "unknown-\0", " \t"])(
    "uses present watch floor %j verbatim and falls back only for None", (floor) => {
      const task = { watch_started_after: floor };
      expect(effectiveTaskWatchFloor(task, timeline)).toBe(rustWatch(task, timeline));
    },
  );

  it("uses the last watched event at a tied suffix, matching Rust max_by_key", () => {
    const events = [watched("te-A-B"), watched("unknown-A-B")];
    expect(effectiveTaskWatchFloor({}, events)).toBe("unknown-A-B");
    expect(effectiveTaskWatchFloor({}, [])).toBeNull();
    expect(effectiveTaskWatchFloor({}, [watched("te-\ue000"), watched("te-\u{10000}")])).toBe("te-\u{10000}");
  });
});

describe("current SPA oracle, retained separately from the canonical source oracle", () => {
  it.each(extraction)("retains trackerUnread extraction for %j", (id, _canonical, legacy) => {
    expect(taskUnreadKey(id)).toBe(legacy);
  });

  // id, mark, current unread-line count, current attention count, source after
  it.each([
    ["tc-A", "te-A", 0, 0, false], ["tc-B", "te-A", 1, 1, true],
    ["tc-A-B", "te-A", 0, 0, true], ["tc-A-Z", "te-A-Y", 0, 1, true],
    ["tc-", "te-", 0, 1, false], ["tc-", null, 0, 1, true],
    ["unknown-B", "te-A", 0, 1, true], ["alpha", "omega", 0, 1, false],
    ["tc-\ue000", "te-\u{10000}", 1, 1, false],
    ["tc-\u{10000}", "te-\ue000", 0, 0, true],
    ["tc-\0", "te-", 1, 1, true], ["tc- ", "te-\t", 1, 1, true],
  ])("records read behavior for id=%j and mark=%j", (id, mark, unread, attention, after) => {
    const row = { key: id, type: "comment", actor: { kind: "agent" } };
    const detail = { task: { read_through: mark }, timeline: [{ id, type: "comment", author: { kind: "agent" } }] };
    expect(taskUnreadReading([row], mark).unreadCount).toBe(unread);
    expect(unreadAgentComments({}, detail)).toHaveLength(attention);
    expect(rustAfter(mark, id)).toBe(after);
    expect(taskMarkerAfter(mark, id)).toBe(after);
  });

  it("records older watch fallback and tie choices independently", () => {
    const rows = [{ type: "event", kind: "watched", key: "te-Z" }];
    expect(taskWatchBaseline(rows, { watch_started_after: "" })).toBe("te-Z");
    expect(effectiveTaskWatchFloor({ watch_started_after: "" }, [watched("te-Z")])).toBe("");
    expect(taskWatchBaseline(rows, { watch_started_after: "unknown-A" })).toBe("te-Z");
    expect(effectiveTaskWatchFloor({ watch_started_after: "unknown-A" }, [watched("te-Z")])).toBe("unknown-A");
    expect(taskWatchBaseline([rows[0], { ...rows[0], key: "tc-Z" }], {})).toBe("te-Z");
    expect(effectiveTaskWatchFloor({}, [watched("te-Z"), watched("tc-Z")])).toBe("tc-Z");
    expect(latestTaskMark("te-A-B", "tc-A-C")).toBeNull();
    expect(newerTaskMarker("te-A-B", "tc-A-C")).toBe("tc-A-C");
  });
});
