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

/** Every `vi.mock` / `vi.doMock` on a local module whose factory is an object
 *  literal, as `{ file, module, names }`. A factory that returns anything else
 *  (a class, a spread of the real module) names nothing to check. */
function mockedModulesIn(file) {
  const source = readFileSync(resolve("test", file), "utf8");
  const found = [];
  const call = /vi\.(?:do)?[Mm]ock\(\s*["'](\.\.?\/[^"']+)["']\s*,\s*(?:async\s*)?\(\)\s*=>\s*\(\{/g;
  for (let match = call.exec(source); match; match = call.exec(source)) {
    const body = objectBodyAt(source, call.lastIndex - 1);
    if (body === null) continue;
    found.push({ file, module: match[1], names: topLevelKeys(body) });
  }
  return found;
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
