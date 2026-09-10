// @vitest-environment jsdom
// An open activity run shows what the agent is doing NOW.
//
// A run's rows scroll in their own little window, eight rows tall. Opening one
// on its oldest row is opening it on the least interesting thing in it, and a
// live run writing into the bottom of a box the reader is looking at the top of
// says nothing at all. So the box follows its newest row — unless the reader
// scrolled up in it, which is them reading, and nothing moves them.

import { describe, expect, it, beforeEach } from "vitest";
import { paintRunsShowingLatest } from "../src/core/activityRunScroll.js";

/** One run's box, whose list states its own geometry. */
function runBox(key, { rows, viewport = 100, rowHeight = 26 }) {
  const box = document.createElement("details");
  box.setAttribute("data-activity-run", key);
  box.innerHTML = '<summary></summary><div class="thread-activity-group-list"></div>';
  const list = box.querySelector(".thread-activity-group-list");
  Object.defineProperty(list, "clientHeight", { get: () => viewport, configurable: true });
  Object.defineProperty(list, "scrollHeight", { get: () => rows.count * rowHeight, configurable: true });
  list.scrollTop = 0;
  return box;
}

describe("paintRunsShowingLatest", () => {
  let scroller;

  beforeEach(() => {
    document.body.innerHTML = "";
    scroller = document.createElement("div");
    document.body.appendChild(scroller);
  });

  const listIn = (key) => scroller.querySelector(`[data-activity-run="${key}"] .thread-activity-group-list`);

  it("opens a run on its newest row", () => {
    const rows = { count: 20 };
    paintRunsShowingLatest(scroller, () => scroller.appendChild(runBox("r1", { rows })));
    expect(listIn("r1").scrollTop).toBe(520);
  });

  it("keeps a run that was at its newest row there as it grows", () => {
    const rows = { count: 20 };
    paintRunsShowingLatest(scroller, () => scroller.appendChild(runBox("r1", { rows })));
    paintRunsShowingLatest(scroller, () => {
      rows.count = 30;
    });
    expect(listIn("r1").scrollTop).toBe(780);
  });

  it("leaves a reader who scrolled up inside the box where they are", () => {
    const rows = { count: 20 };
    paintRunsShowingLatest(scroller, () => scroller.appendChild(runBox("r1", { rows })));
    listIn("r1").scrollTop = 100;
    paintRunsShowingLatest(scroller, () => {
      rows.count = 30;
    });
    expect(listIn("r1").scrollTop).toBe(100);
  });

  it("follows each open run on its own", () => {
    const first = { count: 20 };
    const second = { count: 20 };
    paintRunsShowingLatest(scroller, () => {
      scroller.appendChild(runBox("r1", { rows: first }));
      scroller.appendChild(runBox("r2", { rows: second }));
    });
    listIn("r1").scrollTop = 40;
    paintRunsShowingLatest(scroller, () => {
      first.count = 30;
      second.count = 30;
    });
    expect(listIn("r1").scrollTop).toBe(40);
    expect(listIn("r2").scrollTop).toBe(780);
  });

  it("paints a conversation with no open run and touches nothing", () => {
    let painted = false;
    paintRunsShowingLatest(scroller, () => {
      painted = true;
    });
    expect(painted).toBe(true);
  });

  it("paints and does nothing else when there is no scroller", () => {
    let painted = false;
    paintRunsShowingLatest(null, () => {
      painted = true;
    });
    expect(painted).toBe(true);
  });
});
