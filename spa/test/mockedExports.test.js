// @vitest-environment jsdom
// A mock may only stand in for something that exists.
//
// `vi.mock(path, () => ({ … }))` replaces a module with whatever the factory
// returns, and nothing checks that against the real module. A factory naming an
// export the module does not have is a fiction the whole suite then agrees on:
// every test passes, and the first line of source that imports the invented
// name fails at BUNDLE time, in a production build, long after the tests said
// yes. That is exactly how a `notify` that never existed reached a container
// build on 2026-09-08.
//
// So the factories are read back and checked against the modules they replace.

import { describe, expect, it } from "vitest";
import { readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";

import { srcJsFiles, srcSourceOf } from "./treeFiles.js";

/** Every `vi.mock` / `vi.doMock` on a local module whose factory is an object
 *  literal, as `{ file, module, names }`. A factory that returns anything else
 *  (a class, a spread of the real module) names nothing to check. */
function mockedModulesIn(file) {
  return mockedModulesInSource(file, readFileSync(resolve("test", file), "utf8"));
}

/** The same, over source already in hand — which is what the cases below use to
 *  hold this parser to reading code rather than prose. */
export function mockedModulesInSource(file, rawSource) {
  // Comments only here: the module path this regex captures lives INSIDE a
  // string, so blanking string bodies at this stage would erase the very thing
  // being read. Strings are still walked over, so a `//` inside one (a URL)
  // is not mistaken for a comment.
  const source = blanked(rawSource, { strings: false });
  const found = [];
  const call = /vi\.(?:do)?[Mm]ock\(\s*["'](\.\.?\/[^"']+)["']\s*,\s*(?:async\s*)?\(\)\s*=>\s*\(\{/g;
  for (let match = call.exec(source); match; match = call.exec(source)) {
    const body = objectBodyAt(source, call.lastIndex - 1);
    if (body === null) continue;
    // Strings blanked for the KEY scan, where a quoted phrase carrying a colon
    // would otherwise read as an export the same way prose did.
    const code = blanked(body, { strings: true });
    found.push({ file, module: match[1], names: topLevelKeys(code), code });
  }
  return found;
}

/**
 * The source with every comment blanked to spaces — and string bodies too, on
 * request.
 *
 * Same length throughout, so every offset the caller goes on to use still
 * points where it did, and blanked a character at a time so a brace inside
 * prose cannot close an object literal early either.
 *
 * A factory declares its exports in code. Prose reading "…the way it answers:
 * an array…" declares nothing, and used to be read as an export named
 * `answers` (#36) — the guard failing over a comment. Strings are always
 * walked rather than parsed, so a `//` inside one is not a comment.
 */
function blanked(source, { strings = false } = {}) {
  const out = [...source];
  let at = 0;
  while (at < source.length) {
    const pair = source.slice(at, at + 2);
    if (pair === "//") at = blankComment(out, source, at, "\n", 0);
    else if (pair === "/*") at = blankComment(out, source, at, "*/", 2);
    else if (source[at] === '"' || source[at] === "'" || source[at] === "`") at = passString(out, source, at, strings);
    else at += 1;
  }
  return out.join("");
}

/** Blank [from, end) where `end` is past the terminator, and answer where to
 *  carry on from. Newlines survive so the line structure does. */
function blankComment(out, source, from, terminator, width) {
  const found = source.indexOf(terminator, from + 2);
  const end = found === -1 ? source.length : found + width;
  for (let at = from; at < end; at++) if (source[at] !== "\n") out[at] = " ";
  return end;
}

/** Walk past a quoted run, blanking its CONTENTS only when asked and always
 *  leaving its quotes where they are. */
function passString(out, source, from, blankIt) {
  const quote = source[from];
  let at = from + 1;
  while (at < source.length && source[at] !== quote) {
    if (source[at] === "\\") at += 1;
    if (blankIt && source[at] !== "\n") out[at] = " ";
    at += 1;
  }
  return at + 1;
}

/** The text of the object literal whose `{` is at `open`, brace-matched so a
 *  nested object or an arrow body cannot end it early. */
function objectBodyAt(source, open) {
  let depth = 0;
  for (let at = open; at < source.length; at++) {
    if (source[at] === "{") depth += 1;
    else if (source[at] === "}") {
      depth -= 1;
      if (depth === 0) return source.slice(open + 1, at);
    }
  }
  return null;
}

/** The keys of one object literal's own level: `name:` and shorthand `name,`,
 *  skipping anything nested inside a deeper brace, bracket or paren. */
function topLevelKeys(body) {
  const keys = [];
  let depth = 0;
  let token = "";
  for (const character of body) {
    if ("{[(".includes(character)) depth += 1;
    else if ("}])".includes(character)) depth -= 1;
    else if (depth === 0 && character === ":") {
      const name = token.trim().split(/\s+/).pop();
      if (/^[A-Za-z_$][\w$]*$/.test(name)) keys.push(name);
      token = "";
      continue;
    } else if (depth === 0 && character === ",") {
      token = "";
      continue;
    }
    if (depth === 0) token += character;
  }
  return keys;
}

const MOCKS = readdirSync(resolve("test"))
  .filter((file) => file.endsWith(".test.js"))
  .flatMap(mockedModulesIn)
  .filter((mock) => mock.names.length);

/** Every name `src/` imports from the module at `path`, across the whole tree. */
function namesImportedFromSrc(path) {
  const names = new Set();
  for (const file of srcJsFiles()) {
    const source = srcSourceOf(file);
    const imports = new RegExp(`import\\s*\\{([^}]*)\\}\\s*from\\s*["'][^"']*${path}["']`, "g");
    for (let match = imports.exec(source); match; match = imports.exec(source))
      for (const name of match[1].split(",")) names.add(name.trim().split(/\s+/).pop());
  }
  return [...names].filter(Boolean);
}

// The modules the app's spine asks things of from far outside the suite that
// mounts them: render() asks the terminal manager which machine the shells type
// at, the connection takes them to the device home moved to, and the device
// list asks the connection to catch home up whenever a status changes. A
// stand-in that answers only the half its own suite exercises throws "not a
// function" out of the first render of a route that names a device, which is a
// matter of which cases the suite happens to have. So a stand-in for one of
// these answers everything the app asks it.
const SPINE = ["terminal/manager.js", "connection.js"];

describe("every mocked export exists on the module it replaces", () => {
  it("finds the mocks to check at all", () => {
    // A guard on the guard: a regex that stops matching would otherwise pass by
    // checking nothing.
    expect(MOCKS.length).toBeGreaterThan(20);
  });

  it.each(MOCKS.map((mock) => [`${mock.file} → ${mock.module}`, mock]))("%s", async (_name, mock) => {
    const real = await import(/* @vite-ignore */ resolve("test", mock.module));
    const invented = mock.names.filter((name) => !(name in real));
    expect(invented, `${mock.file} mocks ${mock.module} with exports it does not have`).toEqual([]);
  });
});

/** The source of one top-level key's value, up to the comma that ends it —
 *  commas nested inside parens, braces or brackets do not. */
function valueOf(code, key) {
  const start = new RegExp(`(^|[,{\\s])${key}\\s*:`).exec(code);
  if (!start) return null;
  let depth = 0;
  let value = "";
  for (const character of code.slice(start.index + start[0].length)) {
    if ("{[(".includes(character)) depth += 1;
    else if ("}])".includes(character)) depth -= 1;
    else if (depth === 0 && character === ",") break;
    value += character;
  }
  return value.trim();
}

/**
 * The one mock shape that has actually cost something.
 *
 * `refreshFeed` is a `Promise.all` over the devices it synced, and
 * views/workspaceView.js takes the first element straight out of it
 * (`const [passed] = await refreshFeed(...)`). A stand-in answering `undefined`
 * destructures nothing and throws out of a promise nobody awaits — seven of
 * them at once, passing every assertion, until the run's EXIT CODE was read
 * rather than its summary (#32).
 *
 * Checked against the real module rather than against a house style, so this
 * says what the contract is and not merely what the other files happen to do.
 */
describe("a stand-in for refreshFeed answers the shape the real one does", () => {
  const standIns = MOCKS.filter((mock) => mock.module.endsWith("taskFeed.js") && mock.names.includes("refreshFeed"));

  it("finds the real contract, and everyone standing in for it", async () => {
    const real = await import("../src/core/taskFeed.js");
    // No device is adopted here, so this is the empty case — still an array.
    expect(Array.isArray(await real.refreshFeed())).toBe(true);
    expect(standIns.length).toBeGreaterThan(10);
  });

  it.each(standIns.map((mock) => [mock.file, mock]))("%s", (_name, mock) => {
    const value = valueOf(mock.code, "refreshFeed");
    // Either an array literal here, or a delegation to a spy the file defines —
    // those are held to the same shape by the suite that owns them.
    const answersArray = /\[/.test(value) || /\(\.\.\.\s*args\s*\)/.test(value);
    expect(answersArray, `${mock.file} stands in for refreshFeed with \`${value}\`, which is not an array`).toBe(true);
  });
});

describe("the factory is read as code, not as prose", () => {
  // #36: a comment inside a factory saying "…the way the real one answers: an
  // array…" was read as an export named `answers`, and the guard failed over
  // it. A colon in prose declares nothing.
  const factoryWith = (body) => `vi.mock("../src/core/taskFeed.js", () => ({${body}}));`;
  const keysOf = (body) => mockedModulesInSource("case.test.js", factoryWith(body))[0].names;

  it("reads the real keys", () => {
    expect(keysOf(`subscribeFeed: () => {}, refreshFeed: async () => [],`)).toEqual(["subscribeFeed", "refreshFeed"]);
  });

  it("ignores a colon in a line comment", () => {
    expect(keysOf(`
      // An array, the way the real one answers: a Promise.all over the devices.
      refreshFeed: async () => [],
    `)).toEqual(["refreshFeed"]);
  });

  it("ignores a colon in a block comment, and a brace in one", () => {
    expect(keysOf(`
      /* what it answers: an array. Not { this: a key } either. */
      refreshFeed: async () => [],
    `)).toEqual(["refreshFeed"]);
  });

  it("ignores a colon inside a string", () => {
    expect(keysOf(`title: "the way it answers: fast", refreshFeed: async () => [],`))
      .toEqual(["title", "refreshFeed"]);
  });

  // The module path is itself a string, so the blanking that protects the key
  // scan must not reach the path the mock is FOR.
  it("still reads the module a factory stands in for", () => {
    const [mock] = mockedModulesInSource("case.test.js", factoryWith(`refreshFeed: async () => [],`));
    expect(mock.module).toBe("../src/core/taskFeed.js");
  });

  it("is not fooled by a // inside a string into eating the rest of the line", () => {
    expect(keysOf(`home: "https://example.test/x", refreshFeed: async () => [],`))
      .toEqual(["home", "refreshFeed"]);
  });
});

describe.each(SPINE)("a stand-in for %s answers everything the app asks it", (module) => {
  const asked = namesImportedFromSrc(module);
  const standIns = MOCKS.filter((mock) => mock.module.endsWith(module));

  it("finds what the app asks it, and who stands in for it", () => {
    expect(asked.length).toBeGreaterThan(3);
    expect(standIns.length).toBeGreaterThan(2);
  });

  it.each(standIns.map((mock) => [mock.file, mock]))("%s", (_name, mock) => {
    const unanswered = asked.filter((name) => !mock.names.includes(name));
    expect(unanswered, `${mock.file} stands in for ${module} without it`).toEqual([]);
  });
});
