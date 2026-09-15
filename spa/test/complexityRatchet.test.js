// The eslint complexity ratchet only goes down (CLAUDE.md "Complexity gates").
//
// `// eslint-disable-next-line complexity` is how a function that was already
// over the cap when the gate landed stays in the tree. eslint is happy either
// way once the comment is written, so the count is asserted here: adding one
// fails this test, and retiring one is a deliberate edit of the number below.

import { describe, it, expect } from "vitest";

import { srcJsFiles, srcSourceOf } from "./treeFiles.js";

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
// 69 since cacheSync's snapshot handler became four named steps: the active
// rows, the eviction, the watcher set, and the entities entering it — and
// syncDeviceSnapshot follows one device at a time, so onSnapshot only walks the
// merged snapshot's devices.
// 68 since capture routing is branch-only, so its manual form no longer
// exceeds the cap.
// 67 since each route kind writes its own hash: hashFromRoute looks the writer
// up instead of walking every kind in one chain.
// 66 since a row's project tag is one function both row painters call, rather
// than the same conditional written out in each of them.
// 65 since the rail's projects face lists only workspaces: a project block's
// head has one kind of surface to open and one create button, and its chevron
// is its own helper.
// 62 since a conversation's reference chips are written from one table of the
// fields a reference carries: the render reads it forwards, the wiring reads it
// backwards, and neither walks the fields one `if` at a time. The label a chip
// wears is a list of the fields it can be named by, and the lines it points at
// are their own helper.
// 61 since the pane drawer stopped guarding against chrome it writes itself:
// paneDrawerHtml always emits the scrim and the handle, and both panes always
// hand initPaneDrawer a list column, so the wiring reads straight down instead
// of asking whether each of its own parts is there.
// 59 since core/relayLink.js is gone: the relay is a rendezvous rather than a
// connection (Strict P2P Transport Spec, rule 4), so `createRelayLink` and its
// `connect` — the device-key wait, the pin check, the re-attach test and the
// backoff reconnect in one function each — left the tree with both of their
// exemptions.
const RATCHETED_FUNCTIONS = 59;

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
