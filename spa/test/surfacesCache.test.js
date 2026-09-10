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

  it("carries the snapshot alone, leaving the stamp to the cache", () => {
    expect(surfacesRecord(shells("cargo test"))).toEqual({ surfaces: shells("cargo test") });
  });

  it("reads back the snapshot with the record's own stamp", () => {
    expect(surfacesFromRecord({ at: 1700, value: surfacesRecord(shells("cargo test")) })).toEqual({
      surfaces: shells("cargo test"),
      at: 1700,
    });
  });

  it("stamps an unstamped record as old as time", () => {
    expect(surfacesFromRecord({ value: surfacesRecord(shells("cargo test")) }).at).toBe(0);
  });

  it("answers nothing for a record that is missing, empty or shapeless", () => {
    const shapeless = [
      undefined,
      {},
      { at: 1, value: {} },
      { at: 1, value: { surfaces: null } },
      { at: 1, value: { surfaces: "boom" } },
      { at: 1, value: { surfaces: [{ id: "sh-1" }] } },
    ];
    for (const record of shapeless) expect(surfacesFromRecord(record)).toBe(null);
  });

  it("fingerprints a snapshot that stood still alike, and one that moved apart", () => {
    expect(surfacesFingerprint(shells("cargo test"))).toBe(surfacesFingerprint(shells("cargo test")));
    expect(surfacesFingerprint(shells("cargo test"))).not.toBe(surfacesFingerprint(shells("cargo clippy")));
    expect(surfacesFingerprint(null)).toBe(null);
  });
});
