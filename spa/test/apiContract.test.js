// The SPA half of the contract: the same fixtures/api/v1/ the bridge's
// api_contract.rs test reads. One fixture, two consumers — fixtures hold shapes and advertised names to the bridge and the client.
import { describe, expect, it } from "vitest";
import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { compare, parse, satisfies } from "../src/core/bridgeApi/semver.js";
import { SPA_API_RANGE } from "../src/core/bridgeApi/index.js";
import * as v1 from "../src/core/bridgeApi/v1/index.js";

const fixtureDirectory = fileURLToPath(new URL("../../fixtures/api/v1/", import.meta.url));
const versionsPath = fileURLToPath(new URL("../../fixtures/api/versions.json", import.meta.url));
const apiDirectory = fileURLToPath(new URL("../../fixtures/api/", import.meta.url));
const bridgeApiSourcePath = fileURLToPath(new URL("../../bridge/src/api/mod.rs", import.meta.url));

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

  // A fixture's further examples (a paged `tasks.list`, #85) are results
  // like any other.
  it("parses every further example's result without throwing", () => {
    const examples = methodFixtures.flatMap(({ name, body }) =>
      (body.examples || []).map((example, index) => ({ where: `${name} examples[${index}]`, method: body.method, example })));
    expect(examples.length).toBeGreaterThan(0);
    for (const { where, method, example } of examples) {
      expect(example, where).toHaveProperty("params");
      expect(example, where).toHaveProperty("result");
      expect(() => v1.parseResult(method, example.result), where).not.toThrow();
    }
  });

  // The verb arrived under its own name at 2.0.0 (#190); its paging fields
  // keep the release they first shipped in, under the verb's old name.
  it("dates the tasks.list paging fields at 1.25.0 while keeping the renamed verb's arrival", () => {
    const listing = methodFixtures.find(({ body }) => body.method === "tasks.list").body;
    expect(listing.since).toBe("2.0.0");
    expect(listing.paging).toEqual({
      since: "1.25.0",
      params: ["limit", "cursor"],
      fields: ["next_cursor"],
    });
  });

  it("dates the body ranges at 1.26.0 on each body read, keeping each verb's own arrival (#95)", () => {
    const arrivals = { "fs.read": "1.0.0", "git.diff": "1.0.0", "git.show": "1.0.0", "git.changeset_diff": "1.4.0" };
    for (const [method, since] of Object.entries(arrivals)) {
      const fixture = methodFixtures.find(({ body }) => body.method === method).body;
      expect(fixture.since, method).toBe(since);
      expect(fixture.ranges, method).toEqual({ since: "1.26.0", params: ["range"], fields: ["range"] });
      expect(fixture.examples.some((example) => example.params.range), method).toBe(true);
    }
  });

  it("was introduced within this adapter's major or carried into it, no later than current", () => {
    // The adapter's floor is where it stops serving OLD bridges; a verb that
    // predates the floor is still one it speaks — one a 1.x minor added and
    // 2.0.0 kept is a 2.x verb. What must hold is that no verb is from a
    // major this adapter does not speak yet.
    for (const { name, body } of methodFixtures) {
      expect(Number(String(body.since).split(".")[0]), `${name} since ${body.since}`).toBeLessThanOrEqual(v1.major);
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

// ------------------------------------------------------ when a verb arrived ---

// A new verb filed at the minor it shipped under, not the one it bumped to,
// passes the `since <= current` check above: `tasks.assign` went out saying
// 1.2.0 under a 1.3.0 wire. What catches it is the previous minor's verb list,
// generated from git once (`node scripts/api-verbs-manifest.mjs`) and checked
// in as fixtures/api/verbs-<minor>.json, so nothing here reads history.

const SEMVER = /^\d+\.\d+\.\d+$/;
const RUST_API_VERSION = /^pub const API_VERSION: &str = "([^"]*)";$/m;

/** The version a verb added anywhere in `version`'s release declares: 1.24.0
 *  for 1.24.x, because a patch changes nothing on the wire. */
function releaseOf(version) {
  const { major, minor } = parse(version);
  return `${major}.${minor}.0`;
}

/** Every version a fixture's `since` states, top-level or on a section inside
 *  it (`changes.subscribe`'s `refusal`). Timestamps and commits named `since`
 *  are result data, not versions, and do not match. */
function declaredSinces(value, into = new Set()) {
  if (Array.isArray(value)) {
    for (const item of value) declaredSinces(item, into);
  } else if (value && typeof value === "object") {
    for (const [key, item] of Object.entries(value)) {
      if (key === "since" && typeof item === "string" && SEMVER.test(item)) into.add(item);
      else declaredSinces(item, into);
    }
  }
  return into;
}

/** (a) Every verb with a fixture now and none at the manifest's minor must say
 *  it arrived in the current release. */
function newVerbsFiledAtTheWrongMinor({ methodFixtures, manifest, current }) {
  const before = new Set(manifest.verbs);
  const release = releaseOf(current);
  return methodFixtures
    .filter(({ body }) => !before.has(body.method) && body.since !== release)
    .map(({ body }) => `${body.method} is new since ${manifest.api_version} but says since ${body.since}, not ${release}`);
}

/** (b) The number the bridge speaks, the number the fixtures call current, and
 *  the number the greeting fixture reports are one number. */
function versionDisagreements({ versions, hello, bridgeSource }) {
  const bridge = RUST_API_VERSION.exec(bridgeSource)?.[1];
  const problems = [];
  if (bridge === undefined) problems.push("bridge/src/api/mod.rs declares no `pub const API_VERSION: &str`");
  else if (bridge !== versions.current) problems.push(`the bridge's API_VERSION is ${bridge}, versions.json says ${versions.current}`);
  if (hello.result.api_version !== versions.current) {
    problems.push(`session.hello reports ${hello.result.api_version}, versions.json says ${versions.current}`);
  }
  return problems;
}

/** (c) A capability the greeting announces declares when it arrived: one with
 *  a fixture of its own carries the current release if the previous minor did
 *  not announce it and an older one if it did; one without (a feature, a
 *  legacy verb) that is new needs some fixture, or section of one, declaring
 *  the current release. */
function capabilitiesAtTheWrongMinor({ capabilities, fixtures, manifest, current }) {
  const release = releaseOf(current);
  const announcedBefore = new Set(manifest.capabilities);
  const byMethod = new Map(
    fixtures.filter(({ body }) => typeof body.method === "string").map(({ body }) => [body.method, body]),
  );
  const declaredNow = declaredSinces(fixtures.map(({ body }) => body));
  const problems = [];
  for (const capability of capabilities) {
    const fixture = byMethod.get(capability);
    const isNew = !announcedBefore.has(capability);
    if (fixture && isNew && fixture.since !== release) {
      problems.push(`${capability} is announced since ${release} but its fixture says since ${fixture.since}`);
    } else if (fixture && !isNew && compare(fixture.since, release) >= 0) {
      problems.push(`${capability} was announced at ${manifest.api_version} but its fixture says since ${fixture.since}`);
    } else if (!fixture && isNew && !declaredNow.has(release)) {
      problems.push(`${capability} is new at ${release} and no fixture declares since ${release}`);
    }
  }
  return problems;
}

const current = parse(versions.current);
const manifestNames = readdirSync(apiDirectory).filter((name) => /^verbs-.*\.json$/.test(name));
const manifest = manifestNames.length === 1 ? readJson(apiDirectory + manifestNames[0]) : null;

/** Whether `previous` is the release right before `current`: the minor before
 *  it, or for a new major's first minor, any release of the major before — or,
 *  for a later patch of that first minor, its own first release. */
function directlyPrecedes(previous, current_) {
  const [was, now] = [parse(previous), parse(current_)];
  if (now.minor !== 0) return was.major === now.major && was.minor + 1 === now.minor;
  const firstRelease = was.major === now.major && was.minor === 0 && was.patch < now.patch;
  return was.major + 1 === now.major || firstRelease;
}
const hello = fixtures.find(({ name }) => name === "session.hello.json").body;

describe("when each verb and capability arrived, against the previous minor", () => {
  it("keeps the previous release's manifest and no other", () => {
    expect(manifestNames, "run node scripts/api-verbs-manifest.mjs").toHaveLength(1);
    expect(manifestNames[0]).toBe(`verbs-${releaseOf(manifest.api_version).replace(/\.0$/, "")}.json`);
    expect(directlyPrecedes(manifest.api_version, versions.current), manifest.api_version).toBe(true);
    expect(manifest.verbs.length).toBeGreaterThan(100);
    expect(manifest.capabilities.length).toBeGreaterThan(100);
  });

  it("files every verb new since the previous minor at the current one", () => {
    expect(newVerbsFiledAtTheWrongMinor({ methodFixtures, manifest, current: versions.current })).toEqual([]);
  });

  it("speaks the version versions.json calls current, bridge and greeting both", () => {
    const bridgeSource = readFileSync(bridgeApiSourcePath, "utf8");
    expect(versionDisagreements({ versions, hello, bridgeSource })).toEqual([]);
  });

  it("announces each capability with a fixture that declares when it arrived", () => {
    const problems = capabilitiesAtTheWrongMinor({
      capabilities: hello.result.capabilities,
      fixtures,
      manifest,
      current: versions.current,
    });
    expect(problems).toEqual([]);
  });
});

describe("the arrival checks, on a synthetic violation each", () => {
  it("takes any release of the major before as the one a new major follows", () => {
    expect(directlyPrecedes("1.30.0", "2.0.0")).toBe(true);
    expect(directlyPrecedes("1.29.2", "1.30.0")).toBe(true);
    expect(directlyPrecedes("1.28.0", "1.30.0")).toBe(false);
    expect(directlyPrecedes("1.30.0", "3.0.0")).toBe(false);
    expect(directlyPrecedes("1.30.0", "2.1.0")).toBe(false);
  });

  // Past a new major's first release, its patches are held to that release:
  // the manifest is written from it, as the bridge's contract test requires.
  it("takes a new major's first release as the one its later patches follow", () => {
    expect(directlyPrecedes("2.0.0", "2.0.1")).toBe(true);
    expect(directlyPrecedes("2.0.1", "2.0.1")).toBe(false);
  });

  const previous = { api_version: "1.23.0", verbs: ["a.old"], capabilities: ["a.old", "a.feature"] };
  const fixture = (method, since, extra = {}) => ({ name: `${method}.json`, body: { method, since, ...extra } });

  it("(a) refuses a new verb filed at the previous minor", () => {
    const methodFixtures = [fixture("a.old", "1.0.0"), fixture("a.new", "1.23.0")];
    expect(newVerbsFiledAtTheWrongMinor({ methodFixtures, manifest: previous, current: "1.24.0" })).toEqual([
      "a.new is new since 1.23.0 but says since 1.23.0, not 1.24.0",
    ]);
  });

  it("(a) takes the release, not the patch, as the minor a verb arrived in", () => {
    const methodFixtures = [fixture("a.old", "1.0.0"), fixture("a.new", "1.24.0")];
    expect(newVerbsFiledAtTheWrongMinor({ methodFixtures, manifest: previous, current: "1.24.2" })).toEqual([]);
  });

  it("(b) refuses a bridge constant, or a greeting, that disagrees with versions.json", () => {
    const versions_ = { current: "1.24.0" };
    const agreeing = { result: { api_version: "1.24.0" } };
    const stale = 'pub const API_VERSION: &str = "1.23.0";\n';
    expect(versionDisagreements({ versions: versions_, hello: agreeing, bridgeSource: stale })).toEqual([
      "the bridge's API_VERSION is 1.23.0, versions.json says 1.24.0",
    ]);
    const current_ = 'pub const API_VERSION: &str = "1.24.0";\n';
    expect(
      versionDisagreements({ versions: versions_, hello: { result: { api_version: "1.23.0" } }, bridgeSource: current_ }),
    ).toEqual(["session.hello reports 1.23.0, versions.json says 1.24.0"]);
    expect(versionDisagreements({ versions: versions_, hello: agreeing, bridgeSource: "" })).toEqual([
      "bridge/src/api/mod.rs declares no `pub const API_VERSION: &str`",
    ]);
  });

  it("(c) refuses a capability whose fixture says it arrived at another minor", () => {
    const check = (capabilities, fixtures) =>
      capabilitiesAtTheWrongMinor({ capabilities, fixtures, manifest: previous, current: "1.24.0" });
    expect(check(["a.old", "a.new"], [fixture("a.old", "1.0.0"), fixture("a.new", "1.23.0")])).toEqual([
      "a.new is announced since 1.24.0 but its fixture says since 1.23.0",
    ]);
    expect(check(["a.old"], [fixture("a.old", "1.24.0")])).toEqual([
      "a.old was announced at 1.23.0 but its fixture says since 1.24.0",
    ]);
    expect(check(["a.old", "b.feature"], [fixture("a.old", "1.0.0")])).toEqual([
      "b.feature is new at 1.24.0 and no fixture declares since 1.24.0",
    ]);
    const section = fixture("a.old", "1.0.0", { refusal: { since: "1.24.0" }, result: { since: "2026-09-25T00:00:00Z" } });
    expect(check(["a.old", "a.feature", "b.feature"], [section])).toEqual([]);
  });
});
