// @vitest-environment jsdom
// How the surfaces enter and leave the status row and the conversation column.
//
// A pill that arrives grows into the strip; one that leaves shrinks away and is
// out of the document only once it has finished. A pill's count is a cap at its
// end that grows and shrinks in place, the pill staying where it is. The viewer
// grows upward out of the composer's top line when a kind opens and shrinks
// back before its content is let go — but changing which kind is open is a swap,
// not a departure and an arrival.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { motionBeat, recordAnimations, settleMotion, stopRecordingAnimations } from "./motionRecorder.js";
import { motionSettled } from "../src/core/motion.js";
import { mountAgentSurfaces } from "../src/core/agentSurfaces.js";
import { CHECKLIST_ENTRY_KIND, SHELL_ENTRY_KIND } from "../src/core/agentSurfacesModel.js";
import { surfacesSnapshot } from "./surfacesFixture.js";

vi.mock("../src/core/notify.js", () => ({ notifyError: () => {}, notify: () => {} }));

const SURFACES_KEY = "branch-1:agent-1";

let started = [];
let block = null;

const pillHost = () => document.getElementById("rail-status-pills");
const viewerHost = () => document.getElementById("rail-surfaces-viewer");
const pills = () => [...pillHost().querySelectorAll(".surface-pill")];
const pillOf = (kind) => pillHost().querySelector(`[data-surface-kind="${kind}"]`);
const capOf = (kind) => pillOf(kind).querySelector(".surface-pill-count");
const animationsOn = (element) => started.filter((run) => run.element === element);

const checklistOnly = (state) => ({ checklist: [{ id: "c1", subject: "Land the fold", state }] });

const mount = () => {
  document.body.innerHTML = `<div class="rail-panel">
    <div class="rail-body" id="rail-body"></div>
    <div class="rail-surfaces-viewer" id="rail-surfaces-viewer" hidden></div>
    <div class="rail-composer" id="rail-composer">
      <div class="rail-status" id="rail-status">
        <span class="rail-status-lead" id="rail-status-lead"></span>
        <div class="rail-status-pills" id="rail-status-pills"></div>
        <span class="rail-status-git" id="rail-status-git"></span>
      </div>
    </div>
  </div>`;
  block = mountAgentSurfaces({
    pillHost: pillHost(),
    viewerHost: viewerHost(),
    key: SURFACES_KEY,
    onSendMessage: async () => {},
    onOpenThreadItem: () => {},
  });
  return block;
};

beforeEach(() => {
  globalThis.localStorage.clear();
  started = recordAnimations();
});

afterEach(async () => {
  if (block) block.dispose();
  block = null;
  await motionSettled();
  stopRecordingAnimations();
  started = [];
  document.body.innerHTML = "";
});

describe("a pill arriving and leaving the strip", () => {
  it("grows one that arrives and shrinks one that leaves before it is taken out", async () => {
    const surfaces = mount();
    surfaces.set(checklistOnly("in_progress"));
    await motionBeat();

    const [pill] = pills();
    expect(pill).toBeTruthy();
    expect(animationsOn(pill)[0].keyframes[0]).toHaveProperty("width");
    await settleMotion(started);
    expect(pill.hidden).toBe(false);

    started.length = 0;
    surfaces.set({});
    await motionBeat();

    expect(animationsOn(pill)[0]).toBeTruthy();
    expect(animationsOn(pill)[0].keyframes[1]).toEqual({ width: "0px", opacity: 0 });
    expect(pillHost().contains(pill)).toBe(true);

    await settleMotion(started);
    expect(pillHost().contains(pill)).toBe(false);
  });
});

describe("the cap a pill's count rides in", () => {
  it("sits at the pill's end, behind its label", async () => {
    const surfaces = mount();
    surfaces.set(checklistOnly("in_progress"));
    await settleMotion(started);

    const pill = pillOf(CHECKLIST_ENTRY_KIND);
    expect([...pill.children].map((child) => child.className)).toEqual([
      "surface-pill-label",
      "surface-pill-count",
    ]);
    expect(capOf(CHECKLIST_ENTRY_KIND).textContent).toBe("1");
  });

  it("grows into place when the count arrives and shrinks away when it goes, the pill staying", async () => {
    const surfaces = mount();
    surfaces.set(checklistOnly("pending"));
    await settleMotion(started);

    const pill = pillOf(CHECKLIST_ENTRY_KIND);
    const cap = capOf(CHECKLIST_ENTRY_KIND);
    expect(cap.hidden).toBe(true);

    started.length = 0;
    surfaces.set(checklistOnly("in_progress"));
    await motionBeat();
    expect(animationsOn(cap)[0].keyframes[0]).toEqual({ width: "0px", opacity: 0 });
    await settleMotion(started);
    expect(cap.hidden).toBe(false);
    expect(cap.textContent).toBe("1");

    started.length = 0;
    surfaces.set(checklistOnly("completed"));
    await motionBeat();
    expect(animationsOn(cap)[0].keyframes[1]).toEqual({ width: "0px", opacity: 0 });
    await settleMotion(started);

    expect(cap.hidden).toBe(true);
    expect(pillOf(CHECKLIST_ENTRY_KIND)).toBe(pill);
    expect(pill.contains(cap)).toBe(true);
  });
});

describe("the viewer at the bottom of the conversation", () => {
  it("grows by its height when a kind opens", async () => {
    const surfaces = mount();
    surfaces.set(surfacesSnapshot());
    await settleMotion(started);

    started.length = 0;
    pillOf(SHELL_ENTRY_KIND).click();
    await motionBeat();

    expect(animationsOn(viewerHost())[0].keyframes[0]).toEqual({ height: "0px", opacity: 0 });
    await settleMotion(started);
    expect(viewerHost().hidden).toBe(false);
    expect(viewerHost().querySelector(".surface-shells")).not.toBe(null);
  });

  it("shrinks away before its content is let go", async () => {
    const surfaces = mount();
    surfaces.set(surfacesSnapshot());
    pillOf(SHELL_ENTRY_KIND).click();
    await settleMotion(started);

    started.length = 0;
    pillOf(SHELL_ENTRY_KIND).click();
    await motionBeat();

    expect(animationsOn(viewerHost())[0].keyframes[1]).toEqual({ height: "0px", opacity: 0 });
    expect(viewerHost().querySelector(".surface-shells")).not.toBe(null);

    await settleMotion(started);
    expect(viewerHost().hidden).toBe(true);
    expect(viewerHost().innerHTML).toBe("");
  });

  it("swaps one kind for another without moving at all", async () => {
    const surfaces = mount();
    surfaces.set(surfacesSnapshot());
    pillOf(SHELL_ENTRY_KIND).click();
    await settleMotion(started);

    started.length = 0;
    pillOf(CHECKLIST_ENTRY_KIND).click();
    await motionBeat();

    expect(animationsOn(viewerHost())).toEqual([]);
    expect(viewerHost().hidden).toBe(false);
    expect(viewerHost().querySelector(".surface-checklist")).not.toBe(null);
    expect(viewerHost().querySelector(".surface-shells")).toBe(null);
  });
});
