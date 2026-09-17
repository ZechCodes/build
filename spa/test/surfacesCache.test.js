import { describe, expect, it } from "vitest";
import {
  SURFACES_RECORD_KIND,
  surfacesCacheAddress,
  surfacesFingerprint,
  surfacesFromRecord,
  surfacesRecord,
} from "../src/core/surfacesCache.js";

const shells = (description) => ({ shells: [{ id: "sh-1", description, state: "running", tail: [] }] });

describe("the surfaces record, spelled once for both writers", () => {
  it("addresses one agent's snapshot on one entity", () => {
    expect(surfacesCacheAddress({ deviceId: "dev-1", entityId: "run-3", agentId: "ag-1" })).toEqual({
      deviceId: "dev-1",
      entityId: "run-3",
      kind: SURFACES_RECORD_KIND,
      sub: "ag-1",
    });
  });

  it("addresses the entity's own conversation under an empty sub", () => {
    expect(surfacesCacheAddress({ deviceId: "dev-1", entityId: "run-3", agentId: null }).sub).toBe("");
  });

  it("carries the snapshot with its process generation, leaving the stamp to the cache", () => {
    expect(surfacesRecord(shells("cargo test"), "gen-1")).toEqual({
      surfaces: shells("cargo test"),
      generation: "gen-1",
    });
  });

  it("reads back the snapshot with the record's own stamp", () => {
    expect(surfacesFromRecord({ at: 1700, value: surfacesRecord(shells("cargo test"), "gen-1") }, "gen-1")).toEqual({
      surfaces: shells("cargo test"),
      generation: "gen-1",
      at: 1700,
    });
  });

  it("stamps an unstamped record as old as time", () => {
    expect(surfacesFromRecord({ value: surfacesRecord(shells("cargo test"), "gen-1") }, "gen-1").at).toBe(0);
  });

  it("answers nothing for a record that is missing, empty or shapeless", () => {
    const shapeless = [
      undefined,
      {},
      { at: 1, value: {} },
      { at: 1, value: { surfaces: "boom" } },
      { at: 1, value: { surfaces: [{ id: "sh-1" }] } },
    ];
    for (const record of shapeless) expect(surfacesFromRecord(record, "gen-1")).toBe(null);
  });

  it("reads a generation-scoped null as a whole-snapshot clear", () => {
    expect(surfacesFromRecord({ at: 1700, value: surfacesRecord(null, "gen-1") }, "gen-1")).toEqual({
      surfaces: null,
      generation: "gen-1",
      at: 1700,
    });
  });

  it("rejects legacy, missing and mismatched process generations", () => {
    const snapshot = shells("old process");
    expect(surfacesFromRecord({ value: { surfaces: snapshot } }, "gen-2")).toBe(null);
    expect(surfacesFromRecord({ value: surfacesRecord(snapshot, "gen-1") }, null)).toBe(null);
    expect(surfacesFromRecord({ value: surfacesRecord(snapshot, "gen-1") }, "gen-2")).toBe(null);
  });

  it("marks restored goal and checklist observations stale without changing execution metadata", () => {
    const snapshot = {
      goal: { objective: "Ship it", state: "active" },
      checklist: [{ id: "one", subject: "Test", state: "pending" }],
      shells: [{ id: "sh-1", state: "running" }],
      observations: {
        goal: { support: "supported", freshness: "current", coverage: "complete", observed_at: "then" },
        checklist: { support: "unknown", freshness: "loading" },
        shells: { support: "supported", freshness: "current", coverage: "complete" },
      },
    };
    const seen = surfacesFromRecord({ value: surfacesRecord(snapshot, "gen-1") }, "gen-1");
    expect(seen.surfaces.observations.goal.freshness).toBe("stale");
    expect(seen.surfaces.observations.checklist.freshness).toBe("stale");
    expect(seen.surfaces.observations.shells.freshness).toBe("current");
    expect(snapshot.observations.goal.freshness).toBe("current");
  });

  it("fingerprints a snapshot that stood still alike, and one that moved apart", () => {
    expect(surfacesFingerprint(shells("cargo test"), "gen-1")).toBe(surfacesFingerprint(shells("cargo test"), "gen-1"));
    expect(surfacesFingerprint(shells("cargo test"), "gen-1")).not.toBe(surfacesFingerprint(shells("cargo clippy"), "gen-1"));
    expect(surfacesFingerprint(shells("cargo test"), "gen-1")).not.toBe(surfacesFingerprint(shells("cargo test"), "gen-2"));
    expect(surfacesFingerprint(null, "gen-1")).not.toBe(null);
    expect(surfacesFingerprint(null, null)).toBe(null);
  });
});
