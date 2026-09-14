// The eslint complexity ratchet only goes down (CLAUDE.md "Complexity gates").
//
// `// eslint-disable-next-line complexity` is how a function that was already
// over the cap when the gate landed stays in the tree. eslint is happy either
// way once the comment is written, so the count is asserted here: adding one
// fails this test, and retiring one is a deliberate edit of the number below.

import { describe, it, expect } from "vitest";

import { srcJsFiles, srcSourceOf } from "./srcFiles.js";

// Measured when the gate landed: 78 functions across 39 files under src/.
// 77 since diffFileHtml became a header, a body and a frame.
// 75 since the timeline became keyed rows: threadHtml took its initial message
// and its title from two named helpers, and the row builder split into a
// message row and an activity row.
// 73 since chat state moved behind addressed controllers: agent identity no
// longer migrates global draft maps, and message rendering reads its owned
// thread state instead of branching across ambient maps.
// Main's toolbar/composer extraction and chat state ownership together retire
// four counted functions, with no new exemption added.
// The editable file viewer splits file-selection setup from its async read.
// 69 since the cache syncer follows a device at a time: onSnapshot now only
// walks the merged snapshot's devices, and the work it used to do inline is
// syncDeviceSnapshot.
// 68 since each route kind writes its own hash: hashFromRoute looks the writer
// up instead of walking every kind in one chain.
const RATCHETED_FUNCTIONS = 68;

const DISABLE = "eslint-disable-next-line complexity";
// A block or file-level disable would switch the rule off for everything
// below it without touching the count above: none may exist.
const BLANKET = /eslint-disable(?!-next-line)[^\n]*complexity/;

describe("the complexity ratchet", () => {
  it("is never switched off for a whole block or file", () => {
    const blanket = srcJsFiles().filter((file) => BLANKET.test(srcSourceOf(file)));
    expect(blanket, "a blanket eslint-disable for complexity defeats the ratchet").toEqual([]);
  });

  it("holds at the count measured when the gate landed", () => {
    const found = srcJsFiles().flatMap((file) =>
      srcSourceOf(file)
        .split("\n")
        .flatMap((line, index) => (line.includes(DISABLE) ? [`${file}:${index + 1}`] : [])),
    );
    expect(
      found.length,
      "the complexity ratchet moved — split the function instead of disabling the " +
        "rule, or lower RATCHETED_FUNCTIONS when retiring one",
    ).toBe(RATCHETED_FUNCTIONS);
  });
});
