// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { motionBeat, recordAnimations, settleMotion, stopRecordingAnimations } from "./motionRecorder.js";
import { mountAgentSurfaces } from "../src/core/agentSurfaces.js";
import {
  CHECKLIST_ENTRY_KIND,
  SHELL_ENTRY_KIND,
  WORKFLOW_ENTRY_KIND,
} from "../src/core/agentSurfacesModel.js";
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
  await settleMotion();
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
    await settleMotion();
    expect(pill.hidden).toBe(false);

    started.length = 0;
    surfaces.set({});
    await motionBeat();

    expect(animationsOn(pill)[0]).toBeTruthy();
    expect(animationsOn(pill)[0].keyframes[1]).toEqual({ width: "0px", opacity: 0 });
    expect(pillHost().contains(pill)).toBe(true);

    await settleMotion();
    expect(pillHost().contains(pill)).toBe(false);
  });
});

describe("the cap a pill's count rides in", () => {
  it("sits at the pill's end, behind its label", async () => {
    const surfaces = mount();
    surfaces.set(checklistOnly("in_progress"));
    await settleMotion();

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
    await settleMotion();

    const pill = pillOf(CHECKLIST_ENTRY_KIND);
    const cap = capOf(CHECKLIST_ENTRY_KIND);
    expect(cap.hidden).toBe(true);

    started.length = 0;
    surfaces.set(checklistOnly("in_progress"));
    await motionBeat();
    expect(animationsOn(cap)[0].keyframes[0]).toEqual({ width: "0px", opacity: 0 });
    await settleMotion();
    expect(cap.hidden).toBe(false);
    expect(cap.textContent).toBe("1");

    started.length = 0;
    surfaces.set(checklistOnly("completed"));
    await motionBeat();
    expect(animationsOn(cap)[0].keyframes[1]).toEqual({ width: "0px", opacity: 0 });
    await settleMotion();

    expect(cap.hidden).toBe(true);
    expect(pillOf(CHECKLIST_ENTRY_KIND)).toBe(pill);
    expect(pill.contains(cap)).toBe(true);
  });
});

describe("the rows of an open viewer", () => {
  const shells = (...descriptions) => ({
    shells: descriptions.map((description, index) => ({
      id: `sh${index + 1}`,
      description,
      state: "running",
      tail: [],
    })),
  });

  const shellRows = () => [...viewerHost().querySelectorAll(".surface-running > .surface-row")];

  it("grows one that arrives and shrinks one that leaves before it is taken out", async () => {
    const surfaces = mount();
    surfaces.set(shells("cargo test"));
    pillOf(SHELL_ENTRY_KIND).click();
    await settleMotion();

    started.length = 0;
    surfaces.set(shells("cargo test", "cargo clippy"));
    await motionBeat();

    const arriving = shellRows()[1];
    expect(arriving.textContent).toContain("cargo clippy");
    expect(animationsOn(arriving)[0].keyframes[0]).toEqual({ height: "0px", opacity: 0 });
    await settleMotion();
    expect(arriving.hidden).toBe(false);

    started.length = 0;
    surfaces.set(shells("cargo test"));
    await motionBeat();

    expect(animationsOn(arriving)[0].keyframes[1]).toEqual({ height: "0px", opacity: 0 });
    expect(viewerHost().contains(arriving)).toBe(true);

    await settleMotion();
    expect(viewerHost().contains(arriving)).toBe(false);
  });
});

describe("the phases an open workflow stacks", () => {
  const workflow = (phases) => ({
    workflows: [
      {
        id: "wf-1",
        name: "Review sweep",
        state: "running",
        phases: phases.map((labels, index) => ({
          title: `phase ${index}`,
          agents: labels.map((label) => ({ id: label, label, state: "running" })),
        })),
      },
    ],
  });

  const sections = () => [...viewerHost().querySelectorAll(".surface-phase")];

  const openWorkflowViewer = async (surfaces, phases) => {
    surfaces.set(workflow(phases));
    pillOf(WORKFLOW_ENTRY_KIND).click();
    await settleMotion();
    started.length = 0;
  };

  it("grows a phase that arrives, and a row that arrives inside one already standing", async () => {
    const surfaces = mount();
    await openWorkflowViewer(surfaces, [["reader"]]);

    surfaces.set(workflow([["reader", "skimmer"], ["judge"]]));
    await settleMotion();

    const arrivingRow = sections()[0].querySelector(`[data-key="skimmer"]`);
    const arrivingSection = sections()[1];
    expect(animationsOn(arrivingRow)[0].keyframes[0]).toEqual({ height: "0px", opacity: 0 });
    expect(animationsOn(arrivingSection)[0].keyframes[0]).toEqual({ height: "0px", opacity: 0 });
    expect(arrivingRow.hidden).toBe(false);
    expect(arrivingSection.hidden).toBe(false);
  });

  it("shrinks a phase away before it is taken out of the stack", async () => {
    const surfaces = mount();
    await openWorkflowViewer(surfaces, [["reader"], ["judge"]]);
    const leaving = sections()[1];

    surfaces.set(workflow([["reader"]]));
    await motionBeat();

    expect(animationsOn(leaving)[0].keyframes[1]).toEqual({ height: "0px", opacity: 0 });
    expect(viewerHost().contains(leaving)).toBe(true);

    await settleMotion();
    expect(viewerHost().contains(leaving)).toBe(false);
  });
});

describe("the viewer at the bottom of the conversation", () => {
  it("grows by its height when a kind opens", async () => {
    const surfaces = mount();
    surfaces.set(surfacesSnapshot());
    await settleMotion();

    started.length = 0;
    pillOf(SHELL_ENTRY_KIND).click();
    await motionBeat();

    expect(animationsOn(viewerHost())[0].keyframes[0]).toEqual({ height: "0px", opacity: 0 });
    await settleMotion();
    expect(viewerHost().hidden).toBe(false);
    expect(viewerHost().querySelector(".surface-shells")).not.toBe(null);
  });

  it("shrinks away before its content is let go", async () => {
    const surfaces = mount();
    surfaces.set(surfacesSnapshot());
    pillOf(SHELL_ENTRY_KIND).click();
    await settleMotion();

    started.length = 0;
    pillOf(SHELL_ENTRY_KIND).click();
    await motionBeat();

    expect(animationsOn(viewerHost())[0].keyframes[1]).toEqual({ height: "0px", opacity: 0 });
    expect(viewerHost().querySelector(".surface-shells")).not.toBe(null);

    await settleMotion();
    expect(viewerHost().hidden).toBe(true);
    expect(viewerHost().innerHTML).toBe("");
  });

  it("takes the frame back when the same kind is pressed again mid-shrink", async () => {
    const surfaces = mount();
    surfaces.set(surfacesSnapshot());
    pillOf(SHELL_ENTRY_KIND).click();
    await settleMotion();

    started.length = 0;
    pillOf(SHELL_ENTRY_KIND).click();
    await motionBeat();
    pillOf(SHELL_ENTRY_KIND).click();
    await settleMotion();

    expect(viewerHost().hidden).toBe(false);
    expect(viewerHost().querySelector(".surface-shells")).not.toBe(null);
    expect(pillOf(SHELL_ENTRY_KIND).getAttribute("aria-pressed")).toBe("true");
  });

  it("is hidden and empty when the panel is disposed mid-reveal", async () => {
    const surfaces = mount();
    surfaces.set(surfacesSnapshot());
    await settleMotion();

    pillOf(SHELL_ENTRY_KIND).click();
    await motionBeat();
    surfaces.dispose();
    block = null;
    await settleMotion();

    expect(viewerHost().hidden).toBe(true);
    expect(viewerHost().innerHTML).toBe("");
  });

  it("swaps one kind for another without moving at all", async () => {
    const surfaces = mount();
    surfaces.set(surfacesSnapshot());
    pillOf(SHELL_ENTRY_KIND).click();
    await settleMotion();

    started.length = 0;
    pillOf(CHECKLIST_ENTRY_KIND).click();
    await motionBeat();

    expect(animationsOn(viewerHost())).toEqual([]);
    expect(viewerHost().hidden).toBe(false);
    expect(viewerHost().querySelector(".surface-checklist")).not.toBe(null);
    expect(viewerHost().querySelector(".surface-shells")).toBe(null);
  });
});
