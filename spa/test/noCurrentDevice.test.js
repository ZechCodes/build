// The app has no current device, and nothing under src/ may grow one back.
//
// Every "which machine is this about?" is answered by the registry now: a
// creation surface asks homeContext(), a route surface asks
// routeContext(App.route), a row's verb asks contextFor(row.deviceId). The
// aliases those questions used to be asked through — App.session, App.call,
// App.cacheScope, App.chatRepository, App.offline, App.offlineSince,
// App.modelCatalog — are gone, together with the switcher that wrote them.
//
// A field on a plain object cannot be deleted by the type system, and an
// ambient singleton can be reintroduced in one line, so the ban is read off the
// source: any of those names back under src/ fails here, whichever file writes
// it. Comments are stripped first — prose may still talk about what the app
// used to do, as long as no code does it.

import { describe, expect, it } from "vitest";

import { srcJsFiles, srcSourceOf } from "./srcFiles.js";

// The six aliases plus the harness catalog that sat beside them, all read off
// App itself. `context.call` and `row.offline` are the answers that replaced
// them, so the ban is anchored on `App.`.
const ALIAS = /\bApp\.(session|call|cacheScope|chatRepository|offline|offlineSince|modelCatalog)\b/;

// The functions that existed to keep one device current: the switcher and the
// home-device writer it became, the application-scope pair, the alias copier,
// the feed's scope reset, and the cache scope's ambient accessors.
const RETIRED = [
  "switchDevice",
  "setHomeDevice",
  "adoptApplicationScope",
  "disposeApplicationScope",
  "pointAliasesAt",
  "resetFeedScope",
  "adoptCacheScope",
  "currentCacheScope",
  "setCacheDevice",
  "cacheDeviceId",
];

const RETIRED_NAME = new RegExp(`\\b(${RETIRED.join("|")})\\b`);

/** One file's source with its comments blanked out, line count preserved so a
 *  hit still reports the line it is on. */
function codeOf(file) {
  return srcSourceOf(file)
    .replace(/\/\*[\s\S]*?\*\//g, (block) => block.replace(/[^\n]/g, " "))
    .replace(/\/\/[^\n]*/g, "");
}

/** Every `file:line` under src/ whose code matches, comments not counted. */
function hitsFor(pattern) {
  return srcJsFiles().flatMap((file) =>
    codeOf(file)
      .split("\n")
      .flatMap((line, index) => (pattern.test(line) ? [`${file}:${index + 1} ${line.trim()}`] : [])),
  );
}

describe("no file under spa/src reads a current device", () => {
  it("finds files to scan at all", () => {
    // A guard on the guard: a reader that stopped finding anything would
    // otherwise pass by scanning nothing.
    expect(srcJsFiles().length).toBeGreaterThan(100);
  });

  it("names none of the retired App.* aliases", () => {
    expect(
      hitsFor(ALIAS),
      "ask homeContext(), routeContext(App.route) or contextFor(deviceId) for the machine instead",
    ).toEqual([]);
  });

  it("calls none of the retired current-device functions", () => {
    expect(
      hitsFor(RETIRED_NAME),
      "the switcher and the application-scope singletons are gone; the registry owns every device",
    ).toEqual([]);
  });
});
