// The reference forms an agent can write, read back off prose.
//
// #56. An agent should be able to point at an issue, a workspace, another agent
// or a file without knowing a route. The shapes have to survive ordinary
// writing: a heading, a hex colour, an email address, an npm scope, a version
// specifier and a bare SHA all appear in this project's own notes, and none of
// them is a link.

import { describe, it, expect } from "vitest";

import { referencesIn } from "../src/core/markdownRefs.js";

/** Just the kinds and their fields, which is what a caller acts on. */
const found = (text) => referencesIn(text).map(({ raw, ...rest }) => rest);

describe("what a reference is", () => {
  it("reads an issue by its number", () => {
    expect(found("see #42 for the rest")).toEqual([{ kind: "issue", number: 42, start: 4, end: 7 }]);
  });

  it("reads a workspace by name or id", () => {
    expect(found("@workspace:issues-board is cut").map((r) => [r.kind, r.name]))
      .toEqual([["workspace", "issues-board"]]);
    expect(found("@workspace:1aa00c7a-5df4").map((r) => r.name)).toEqual(["1aa00c7a-5df4"]);
  });

  it("reads an agent by id", () => {
    expect(found("ask @agent:agent-01M2ZQ about it").map((r) => [r.kind, r.id]))
      .toEqual([["agent", "agent-01M2ZQ"]]);
  });

  it("reads a file in a workspace", () => {
    expect(found("[[issues-board:bridge/src/mcp.rs]]").map((r) => [r.kind, r.workspace, r.path]))
      .toEqual([["file", "issues-board", "bridge/src/mcp.rs"]]);
  });

  it("reads a line, and takes the first of a range", () => {
    expect(found("[[ws:src/a.js#L10]]").map((r) => r.line)).toEqual([10]);
    // The Files route carries one line, so a range opens at its start.
    expect(found("[[ws:src/a.js#L10-L20]]").map((r) => r.line)).toEqual([10]);
  });

  it("finds several in one line, in the order they were written", () => {
    expect(found("#1 then @workspace:w then [[w:a.js]]").map((r) => r.kind))
      .toEqual(["issue", "workspace", "file"]);
  });
});

describe("what is not a reference", () => {
  // Each of these appears in this repo's own prose. None of them is a link.
  it("leaves ordinary writing alone", () => {
    for (const text of [
      "# A heading",              // a heading needs the space; #42 has none
      "## Another",
      "the colour is #fff",       // hex colours are letters
      "#aabbcc on the border",
      "write to hi@zech.codes",   // email has no leading @
      "install @anthropic-ai/sdk", // an npm scope: slash, no colon
      "we pin @build/secure-transport",
      "run gitleaks@8.30.1",      // a version specifier, not a commit
      "mise exec uv@latest",
      "vitest@4.1.9 is current",
      "rebased onto b8ce4ee9",    // a bare SHA is prose
      "see commit 4c81c037 for it",
      "an array[0] and [one] bracket",
      "issue number 42 in words",
    ]) {
      expect([text, found(text)]).toEqual([text, []]);
    }
  });

  it("wants a boundary before a number, so a fragment is not an issue", () => {
    expect(found("file.js#42")).toEqual([]);
    expect(found("abc#42")).toEqual([]);
  });

  it("wants both brackets, and something on each side of the colon", () => {
    for (const text of ["[[ws:a.js]", "[ws:a.js]]", "[[ws:]]", "[[:a.js]]", "[[a.js]]"]) {
      expect([text, found(text)]).toEqual([text, []]);
    }
  });

  it("wants a keyword after the at sign", () => {
    for (const text of ["@issues-board", "@workspace", "@workspace:", "@agent:"]) {
      expect([text, found(text)]).toEqual([text, []]);
    }
  });
});
