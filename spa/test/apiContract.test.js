// The SPA half of the contract: the same fixtures/api/v1/ the bridge's
// api_contract.rs test reads. One fixture, two consumers — fixtures hold shapes and advertised names to the bridge and the client.
import { describe, expect, it } from "vitest";
import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { compare, satisfies } from "../src/core/bridgeApi/semver.js";
import { SPA_API_RANGE } from "../src/core/bridgeApi/index.js";
import * as v1 from "../src/core/bridgeApi/v1/index.js";

const fixtureDirectory = fileURLToPath(new URL("../../fixtures/api/v1/", import.meta.url));
const versionsPath = fileURLToPath(new URL("../../fixtures/api/versions.json", import.meta.url));

const readJson = (path) => JSON.parse(readFileSync(path, "utf8"));
const versions = readJson(versionsPath);

const fixtureNames = readdirSync(fixtureDirectory)
  .filter((name) => name.endsWith(".json"))
  .sort();
const fixtures = fixtureNames.map((name) => ({ name, body: readJson(fixtureDirectory + name) }));
const methodFixtures = fixtures.filter(({ body }) => typeof body.method === "string");

// The events fixture is the one file in the directory that names no method:
// one example per push the bridge sends on a session.
const eventExamples = fixtures
  .filter(({ body }) => Array.isArray(body.events))
  .flatMap(({ name, body }) => body.events.map((event, index) => ({ where: `${name}[${index}]`, event })));

describe("the v1 adapter against fixtures/api/v1", () => {
  it("finds fixtures at all", () => {
    expect(methodFixtures.length).toBeGreaterThan(100);
  });

  it("declares a range that admits versions.json's current", () => {
    expect(satisfies(versions.current, v1.range)).toBe(true);
    expect(versions.supported_majors).toContain(v1.major);
  });

  /**
   * The gate, not the adapter.
   *
   * `SPA_API_RANGE` is what this build declares in `session.hello` and what
   * decides whether a bridge is served at all; `v1.range` is one adapter's own
   * claim. They are written separately, so the check above can pass while the
   * gate has drifted — and a bridge outside the gate is not a degraded client,
   * it is a dark one.
   *
   * Nothing on either side of the wire can see this by itself: the BRIDGE
   * decides the number and the CLIENT decides what the number costs. The shared
   * fixture is the one place both are visible, which is why the assertion lives
   * here and reads `versions.current` rather than naming a version — a copy of
   * the other side's constant would be worse than no test at all.
   *
   * This is what would have caught the tracker's 1.2.0 → 1.3.0 renumber if it
   * had gone the other way, on the side that knows the range, in a suite
   * instead of on a deploy.
   */
  it("gates on a range that admits versions.json's current too", () => {
    expect(satisfies(versions.current, SPA_API_RANGE)).toBe(true);
  });

  it("names each fixture after the method inside it", () => {
    for (const { name, body } of methodFixtures) {
      expect(`${body.method}.json`).toBe(name);
    }
  });

  it("parses every fixture's result without throwing", () => {
    for (const { name, body } of methodFixtures) {
      expect(body, `${name} has no result`).toHaveProperty("result");
      expect(() => v1.parseResult(body.method, body.result), name).not.toThrow();
    }
  });

  it("was introduced within this adapter's major, no later than current", () => {
    // The adapter's floor is where it stops serving OLD bridges; a verb that
    // predates the floor is still one it speaks. What must hold is the major.
    for (const { name, body } of methodFixtures) {
      expect(Number(String(body.since).split(".")[0]), `${name} since ${body.since}`).toBe(v1.major);
      expect(compare(body.since, versions.current), `${name} since ${body.since}`).toBeLessThanOrEqual(0);
    }
  });

  it("refuses with codes the adapter knows", () => {
    for (const { name, body } of methodFixtures) {
      for (const code of body.errors || []) {
        expect(v1.ERROR_CODES, `${name} refuses with ${code}`).toContain(code);
      }
    }
  });

  it("parses every event example the fixtures carry", () => {
    // Never vacuous: the examples exist, and every one of them parses.
    expect(eventExamples.length).toBeGreaterThan(0);
    for (const { where, event } of eventExamples) {
      expect(v1.parseEvent(event), where).not.toBe(null);
    }
  });
});
