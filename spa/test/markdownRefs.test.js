// The reference forms an agent can write, read back off prose.
//
// #56. An agent should be able to point at a task, a workspace, another agent
// or a file without knowing a route. The shapes have to survive ordinary
// writing: a heading, a hex colour, an email address, an npm scope, a version
// specifier and a bare SHA all appear in this project's own notes, and none of
// them is a link.

import { describe, it, expect } from "vitest";

import { referencesIn } from "../src/core/markdownRefs.js";

/** Just the kinds and their fields, which is what a caller acts on. */
const found = (text) => referencesIn(text).map(({ raw, ...rest }) => rest);

describe("what a reference is", () => {
  it("reads a task by its number", () => {
    expect(found("see #42 for the rest")).toEqual([{ kind: "task", number: 42, start: 4, end: 7 }]);
  });

  it("reads a comment on a task without taking a plain task's suffix", () => {
    expect(found("see #42/c/tc-7 and #43").map((r) => [r.kind, r.number, r.commentId || null]))
      .toEqual([["task", 42, "tc-7"], ["task", 43, null]]);
  });

  it("reads a workspace by name or id", () => {
    expect(found("@workspace:tasks-board is cut").map((r) => [r.kind, r.name]))
      .toEqual([["workspace", "tasks-board"]]);
    expect(found("@workspace:1aa00c7a-5df4").map((r) => r.name)).toEqual(["1aa00c7a-5df4"]);
  });

  it("reads an agent by id", () => {
    expect(found("ask @agent:agent-01M2ZQ about it").map((r) => [r.kind, r.id]))
      .toEqual([["agent", "agent-01M2ZQ"]]);
  });

  it("reads a file in a workspace", () => {
    expect(found("[[tasks-board:bridge/src/mcp.rs]]").map((r) => [r.kind, r.workspace, r.path]))
      .toEqual([["file", "tasks-board", "bridge/src/mcp.rs"]]);
  });

  it("reads short and full commit SHAs", () => {
    const full = `b8ce4ee9${"1".repeat(32)}`;
    expect(found(`[[ws:commit:b8ce4ee9]] [[ws:commit:${full}]]`)
      .map((r) => [r.kind, r.sha])).toEqual([
        ["commit", "b8ce4ee9"], ["commit", full],
      ]);
  });

  it("reads a line, and takes the first of a range", () => {
    expect(found("[[ws:src/a.js#L10]]").map((r) => r.line)).toEqual([10]);
    // The Files route carries one line, so a range opens at its start.
    expect(found("[[ws:src/a.js#L10-L20]]").map((r) => r.line)).toEqual([10]);
  });

  // Both of these were wrong when the first caller wired a resolver to this
  // (#63), and neither could be seen from here: with nothing resolving, every
  // reference rendered as its own words either way.
  it("ends a name before the sentence's full stop, not after it", () => {
    expect(found("cut on @workspace:tasks-spa.").map((r) => r.name)).toEqual(["tasks-spa"]);
    expect(found("ask @agent:agent-01M2ZQ.").map((r) => r.id)).toEqual(["agent-01M2ZQ"]);
    // A dot INSIDE a name is part of it — a workspace may be called one.
    expect(found("@workspace:build.web is cut").map((r) => r.name)).toEqual(["build.web"]);
  });

  // The positions are what a caller splices at, so a start that is off by the
  // length of the workspace name cuts the anchor out of the middle of the line.
  it("starts a bracketed reference at its brackets", () => {
    const [reference] = referencesIn("see [[tasks-spa:src/a.js]] for it");
    expect([reference.start, reference.raw]).toEqual([4, "[[tasks-spa:src/a.js]]"]);
  });

  it("finds several in one line, in the order they were written", () => {
    expect(found("#1 then @workspace:w then [[w:a.js]]").map((r) => r.kind))
      .toEqual(["task", "workspace", "file"]);
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
      "write to hi@example.codes", // email has no leading @
      "install @anthropic-ai/sdk", // an npm scope: slash, no colon
      "we pin @build/secure-transport",
      "run gitleaks@8.30.1",      // a version specifier, not a commit
      "mise exec uv@latest",
      "vitest@4.1.9 is current",
      "rebased onto b8ce4ee9",    // a bare SHA is prose
      "see commit 4c81c037 for it",
      "an array[0] and [one] bracket",
      "task number 42 in words",
      "file.js#42/c/tc-1",
      "[[ws:commit:not-a-sha]]",
      "[[ws:commit:b8c]]", // a commit-looking path that is too short
    ]) {
      expect([text, found(text)]).toEqual([text, []]);
    }
  });

  it("wants a boundary before a number, so a fragment is not a task", () => {
    expect(found("file.js#42")).toEqual([]);
    expect(found("abc#42")).toEqual([]);
  });

  it("wants both brackets, and something on each side of the colon", () => {
    for (const text of ["[[ws:a.js]", "[ws:a.js]]", "[[ws:]]", "[[:a.js]]", "[[a.js]]"]) {
      expect([text, found(text)]).toEqual([text, []]);
    }
  });

  it("wants a keyword after the at sign", () => {
    for (const text of ["@tasks-board", "@workspace", "@workspace:", "@agent:"]) {
      expect([text, found(text)]).toEqual([text, []]);
    }
  });
});
