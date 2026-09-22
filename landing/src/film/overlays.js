// The close-ups: HTML panels that sit on a device's screen, projected every
// frame from the stage's corners, and the beats that animate what is on them.
// Each panel's state (visible, lift, region origin) is tweened by the master
// timeline like any other value; update() turns state into a matrix3d.
import gsap from "gsap";
import { TEXTURE_SIZES, at, span } from "./acts.js";
import { flatQuad, panelTransform } from "../stage/overlay.js";

function numbers(value) {
  return String(value || "").trim().split(/\s+/).map(Number);
}

function readPanel(element) {
  const [x, y, width, height] = numbers(element.dataset.panelRegion);
  const flat = element.dataset.panelFlat ? numbers(element.dataset.panelFlat) : null;
  // The matrix maps a width×height box; the element must be exactly that box.
  element.style.width = `${width}px`;
  element.style.height = `${height}px`;
  return {
    element,
    device: element.dataset.panelDevice,
    width,
    height,
    flat: flat ? { fx: flat[0], fy: flat[1], fw: flat[2] } : null,
    state: { visible: 0, lift: 0, x, y },
  };
}

function typeInto(tl, element, from, until, ease = "none") {
  const text = element.dataset.type || "";
  const state = { chars: 0 };
  element.textContent = "";
  tl.to(state, {
    chars: text.length,
    duration: until - from,
    ease,
    snap: "chars",
    onUpdate: () => { element.textContent = text.slice(0, state.chars); },
  }, from);
}

// A panel's state object fades through `visible`; a DOM element through
// autoAlpha. Either, or a list of either.
function fade(tl, target, value, from, until, ease) {
  const targets = [].concat(target);
  const states = targets.filter((candidate) => candidate && !(candidate instanceof Element) && "visible" in candidate);
  const elements = targets.filter((candidate) => !states.includes(candidate));
  const duration = Math.max(until - from, 0.01);
  if (states.length) tl.to(states, { visible: value, duration, ease }, from);
  if (elements.length) tl.to(elements, { autoAlpha: value, duration, ease }, from);
}

function show(tl, target, from, until) {
  fade(tl, target, 1, from, until, "power2.out");
}

function hide(tl, target, from, until) {
  fade(tl, target, 0, from, until, "power2.in");
}

function caret(tl, panel, name, from, until) {
  const element = panel.querySelector(`[data-caret="${name}"]`);
  if (!element) return;
  show(tl, element, from, from + 0.01);
  hide(tl, element, until, until + 0.01);
}

// A discrete change at one moment that scrubbing back undoes: new text, a
// class gained (or, with `off`, a class lost), or both.
function flip(tl, element, time, { text, className, off = false } = {}) {
  const before = element.textContent;
  const apply = (forward) => {
    if (text !== undefined) element.textContent = forward ? text : before;
    if (className) element.classList.toggle(className, off ? !forward : forward);
  };
  tl.to(element, { duration: 0.01, onStart: () => apply(true), onReverseComplete: () => apply(false) }, time);
}

// A button press: down, then up.
function press(tl, button, from, until) {
  const half = (until - from) / 2;
  tl.to(button, { "--pressed": 1, duration: half, ease: "power2.out" }, from);
  tl.to(button, { "--pressed": 0, duration: half }, from + half);
}

// Act 1 and 2: the laptop has finished its push before the tab switches to
// the test file; a test types in over the scroll, the gutter marks both
// lines, the status bar ticks to "2 changes"; then a reading hold.
function editorBeats(tl, panels) {
  const editor = panels.editor;
  const status = panels.status;
  const lines = editor.element.querySelectorAll("[data-typed-line]");
  const typed = editor.element.querySelectorAll("[data-type]");
  gsap.set(lines, { "--mark": 0 });
  show(tl, editor.state, at(1, 0.18), at(1, 0.21));
  caret(tl, editor.element, "1", at(1, 0.21), at(1, 0.43));
  typeInto(tl, typed[0], at(1, 0.22), at(1, 0.43));
  tl.to(lines[0], { "--mark": 1, duration: span(1, 0, 0.02) }, at(1, 0.43));
  caret(tl, editor.element, "2", at(1, 0.44), at(1, 0.65));
  typeInto(tl, typed[1], at(1, 0.45), at(1, 0.63));
  typeInto(tl, typed[2], at(1, 0.63), at(1, 0.65));
  tl.to([lines[1], lines[2]], { "--mark": 1, duration: span(1, 0, 0.02) }, at(1, 0.65));
  show(tl, status.state, at(1, 0.66), at(1, 0.7));
  hide(tl, editor.state, at(2, 0.9), at(3, 0));
  hide(tl, status.state, at(2, 0.9), at(3, 0));
}

// Act 3: the card lifts out of Ready, gets its assignee, moves to In progress
// with its branch, and is back in the board before the laptop moves on.
function issueBeats(tl, panels) {
  const issue = panels.issue;
  const hole = panels.hole;
  const card = issue.element;
  const none = card.querySelector('[data-assignee="none"]');
  const agent = card.querySelector('[data-assignee="agent"]');
  const ready = card.querySelector('[data-column="ready"]');
  const progress = card.querySelector('[data-column="progress"]');
  const branch = card.querySelector("[data-branch-line]");
  gsap.set([agent, progress, branch], { autoAlpha: 0 });
  gsap.set(branch, { height: 0 });
  show(tl, issue.state, at(3, 0.18), at(3, 0.2));
  tl.to(issue.state, { lift: 1, duration: span(3, 0.2, 0.34), ease: "power3.inOut" }, at(3, 0.2));
  show(tl, hole.state, at(3, 0.2), at(3, 0.26));
  hide(tl, none, at(3, 0.35), at(3, 0.4));
  show(tl, agent, at(3, 0.38), at(3, 0.48));
  hide(tl, ready, at(3, 0.5), at(3, 0.55));
  show(tl, progress, at(3, 0.53), at(3, 0.6));
  tl.to(branch, { autoAlpha: 1, height: "auto", duration: span(3, 0.58, 0.68), ease: "power2.out" }, at(3, 0.58));
  tl.to(branch, { autoAlpha: 0, height: 0, duration: span(3, 0.72, 0.76), ease: "power2.in" }, at(3, 0.72));
  tl.to(issue.state, { lift: 0, x: 892, y: 362, duration: span(3, 0.74, 0.85), ease: "power3.inOut" }, at(3, 0.74));
  hide(tl, [issue.state, hole.state], at(3, 0.98), at(4, 0.01));
}

// Act 4: one row becomes three, the counter climbs; from then on only the
// implementing agent's status changes, with the phone's question and answer.
function teamBeats(tl, panels) {
  const team = panels.team;
  const rows = team.element.querySelectorAll("[data-team-row]");
  const count = team.element.querySelector("[data-running-count]");
  const status = team.element.querySelector('[data-row-status="implement"]');
  gsap.set([rows[1], rows[2]], { autoAlpha: 0, x: 40 });
  show(tl, team.state, at(4, 0), at(4, 0.01));
  const counter = { value: 1 };
  tl.to(rows[1], { autoAlpha: 1, x: 0, duration: span(4, 0.1, 0.18), ease: "power3.out" }, at(4, 0.1));
  tl.to(rows[2], { autoAlpha: 1, x: 0, duration: span(4, 0.2, 0.28), ease: "power3.out" }, at(4, 0.2));
  tl.to(counter, { value: 3, duration: span(4, 0.1, 0.28), snap: "value", onUpdate: () => { count.textContent = String(counter.value); } }, at(4, 0.1));
  flip(tl, status, at(4, 0.45), { text: "Waiting", className: "waiting" });
  flip(tl, status, at(4, 0.75), { text: "Working", className: "waiting", off: true });
  hide(tl, team.state, at(4, 0.96), at(5, 0.0));
}

// The builder's canvas, in the panel's own pixels: nodes are 150 wide on a
// 196 pitch four up and a 186 pitch five up, as the fixture draws them, and
// an arrow sits in each gap.
const NODE_WIDTH = 150;
const NODE_HEIGHT = 125;
const NODE_TOP = 282;
const PALETTE_TEST = { x: -190, y: 85 };
const FOUR_UP = [108, 304, 500, 696];
const FIVE_UP = [28, 214, 400, 586, 772];
const ARROW_WIDTH = 16;

function arrowX(fromX, toX) {
  return (fromX + NODE_WIDTH + toX) / 2 - ARROW_WIDTH / 2;
}

function nodeCentreY() {
  return NODE_TOP + NODE_HEIGHT / 2;
}

// Act 5: the canvas lifts out, a Test node is dragged in on a shallow arc and
// settles between Implement and Review before its arrows draw; the pending
// handoff runs to the gate, dimming each finished node as it passes.
function builderBeats(tl, panels) {
  const builder = panels.builder;
  const root = builder.element;
  const node = (name) => root.querySelector(`[data-node="${name}"]`);
  const arrow = (name) => root.querySelector(`[data-arrow="${name}"]`);
  const status = (name) => root.querySelector(`[data-node-status="${name}"]`);
  const pulse = root.querySelector("[data-pulse]");
  const caption = root.querySelector("[data-canvas-caption]");
  const layout = { implement: 0, review: 1, gate: 2, merge: 3 };
  for (const [name, index] of Object.entries(layout)) gsap.set(node(name), { x: FOUR_UP[index], y: NODE_TOP });
  gsap.set(node("test"), { ...PALETTE_TEST, autoAlpha: 0, scale: 0.6 });
  gsap.set(arrow("implement-review"), { x: arrowX(FOUR_UP[0], FOUR_UP[1]), y: nodeCentreY() });
  gsap.set(arrow("review-gate"), { x: arrowX(FOUR_UP[1], FOUR_UP[2]), y: nodeCentreY() });
  gsap.set(arrow("gate-merge"), { x: arrowX(FOUR_UP[2], FOUR_UP[3]), y: nodeCentreY() });
  gsap.set(arrow("implement-test"), { x: arrowX(FIVE_UP[0], FIVE_UP[1]), y: nodeCentreY(), autoAlpha: 0 });
  gsap.set(arrow("test-review"), { x: arrowX(FIVE_UP[1], FIVE_UP[2]), y: nodeCentreY(), autoAlpha: 0 });
  gsap.set(pulse, { autoAlpha: 0 });
  gsap.set(caption, { autoAlpha: 0, y: 10 });

  show(tl, builder.state, at(5, 0.12), at(5, 0.15));
  tl.to(builder.state, { lift: 1, duration: span(5, 0.15, 0.22), ease: "power3.inOut" }, at(5, 0.15));

  // The drag: the node leaves the palette, grows to size, rises a little and
  // lands in the gap the others open for it.
  tl.to(node("test"), { autoAlpha: 1, scale: 0.8, duration: span(5, 0.35, 0.38), ease: "power2.out" }, at(5, 0.35));
  tl.to(node("test"), { x: FIVE_UP[1], scale: 1, duration: span(5, 0.38, 0.5), ease: "power2.inOut" }, at(5, 0.38));
  tl.to(node("test"), { keyframes: [{ y: PALETTE_TEST.y - 40, duration: span(5, 0.38, 0.42), ease: "power1.out" }, { y: NODE_TOP, duration: span(5, 0.42, 0.5), ease: "power2.inOut" }] }, at(5, 0.38));
  hide(tl, arrow("implement-review"), at(5, 0.4), at(5, 0.44));
  const shift = { duration: span(5, 0.42, 0.5), ease: "power3.inOut" };
  tl.to(node("implement"), { x: FIVE_UP[0], ...shift }, at(5, 0.42));
  tl.to(node("review"), { x: FIVE_UP[2], ...shift }, at(5, 0.42));
  tl.to(node("gate"), { x: FIVE_UP[3], ...shift }, at(5, 0.42));
  tl.to(node("merge"), { x: FIVE_UP[4], ...shift }, at(5, 0.42));
  tl.to(arrow("review-gate"), { x: arrowX(FIVE_UP[2], FIVE_UP[3]), ...shift }, at(5, 0.42));
  tl.to(arrow("gate-merge"), { x: arrowX(FIVE_UP[3], FIVE_UP[4]), ...shift }, at(5, 0.42));
  // Settled: now the new arrows, and the numbers.
  show(tl, arrow("implement-test"), at(5, 0.53), at(5, 0.56));
  show(tl, arrow("test-review"), at(5, 0.56), at(5, 0.59));
  flip(tl, root.querySelector('[data-node-index="review"]'), at(5, 0.53), { text: "3" });
  flip(tl, root.querySelector('[data-node-index="gate"]'), at(5, 0.53), { text: "4" });
  flip(tl, root.querySelector('[data-node-index="merge"]'), at(5, 0.53), { text: "5" });

  // The handoff runs on the new path and stops at the gate. A node the pulse
  // has left is finished, and finished is quiet.
  const y = nodeCentreY();
  tl.set(pulse, { x: FIVE_UP[0] + NODE_WIDTH, y, autoAlpha: 1 }, at(5, 0.6));
  tl.to(pulse, { x: FIVE_UP[1] + NODE_WIDTH / 2, duration: span(5, 0.6, 0.64), ease: "power1.inOut" }, at(5, 0.6));
  flip(tl, node("implement"), at(5, 0.61), { className: "settled" });
  flip(tl, status("test"), at(5, 0.64), { text: "Passed ✓" });
  flip(tl, node("test"), at(5, 0.64), { className: "passed" });
  tl.to(pulse, { x: FIVE_UP[2] + NODE_WIDTH / 2, duration: span(5, 0.65, 0.69), ease: "power1.inOut" }, at(5, 0.65));
  flip(tl, node("test"), at(5, 0.66), { className: "settled" });
  flip(tl, status("review"), at(5, 0.69), { text: "Done" });
  flip(tl, node("review"), at(5, 0.69), { className: "passed" });
  tl.to(pulse, { x: FIVE_UP[3] - 10, duration: span(5, 0.7, 0.74), ease: "power1.inOut" }, at(5, 0.7));
  flip(tl, node("review"), at(5, 0.71), { className: "settled" });
  flip(tl, status("gate"), at(5, 0.74), { text: "Waiting for you" });
  flip(tl, node("gate"), at(5, 0.74), { className: "waiting" });
  tl.to(caption, { autoAlpha: 1, y: 0, duration: span(5, 0.76, 0.81), ease: "power2.out" }, at(5, 0.76));
  tl.to(builder.state, { lift: 0, duration: span(5, 0.85, 1), ease: "power3.inOut" }, at(5, 0.85));
  hide(tl, builder.state, at(6, 0), at(6, 0.03));
}

// Act 6: gutter highlights, author chips, one line added to the diff, stage,
// a hold on the finished message, commit; the graph gains the visitor's
// commit and the working tree is clean.
function gitBeats(tl, panels) {
  const git = panels.git;
  const root = git.element;
  const hunkAgent = root.querySelector('[data-hunk="agent"]');
  const hunkYou = root.querySelector('[data-hunk="you"]');
  const chipAgent = root.querySelector('[data-chip="agent"]');
  const chipYou = root.querySelector('[data-chip="you"]');
  const humanLine = root.querySelector("[data-human-line]");
  const humanText = humanLine.querySelector("[data-type]");
  const diffAdd = root.querySelector("[data-diff-add]");
  const closeNumber = root.querySelector("[data-close-number]");
  const stageAll = root.querySelector("[data-stage-all]");
  const treeLabel = root.querySelector("[data-tree-label]");
  const treeCount = root.querySelector("[data-tree-count]");
  const changedFiles = root.querySelectorAll("[data-changed-file]");
  const cleanTree = root.querySelector("[data-clean-tree]");
  const commitInput = root.querySelector("[data-commit-input] [data-type]");
  const commitButton = root.querySelector("[data-commit-button]");
  const newCommit = root.querySelector("[data-new-commit]");
  gsap.set([chipAgent, chipYou], { autoAlpha: 0, x: 8 });
  gsap.set(humanLine, { height: 0, autoAlpha: 0 });
  gsap.set(newCommit, { height: 0, autoAlpha: 0 });
  gsap.set(cleanTree, { autoAlpha: 0 });
  gsap.set([hunkAgent, hunkYou], { "--lit": 0 });

  show(tl, git.state, at(6, 0.02), at(6, 0.05));
  tl.to(git.state, { lift: 1, duration: span(6, 0.1, 0.2), ease: "power3.inOut" }, at(6, 0.1));
  tl.to(hunkAgent, { "--lit": 1, duration: span(6, 0.2, 0.26) }, at(6, 0.2));
  tl.to(chipAgent, { autoAlpha: 1, x: 0, duration: span(6, 0.22, 0.28), ease: "power2.out" }, at(6, 0.22));
  tl.to(hunkYou, { "--lit": 1, duration: span(6, 0.3, 0.36) }, at(6, 0.3));
  tl.to(chipYou, { autoAlpha: 1, x: 0, duration: span(6, 0.32, 0.38), ease: "power2.out" }, at(6, 0.32));
  tl.to(humanLine, { height: "auto", autoAlpha: 1, duration: span(6, 0.4, 0.43), ease: "power2.out" }, at(6, 0.4));
  caret(tl, root, "human", at(6, 0.4), at(6, 0.55));
  typeInto(tl, humanText, at(6, 0.42), at(6, 0.54));
  flip(tl, diffAdd, at(6, 0.54), { text: "+3" });
  flip(tl, closeNumber, at(6, 0.54), { text: "15" });
  press(tl, stageAll, at(6, 0.56), at(6, 0.62));
  flip(tl, treeLabel, at(6, 0.59), { text: "Staged" });
  caret(tl, root, "commit", at(6, 0.62), at(6, 0.73));
  typeInto(tl, commitInput, at(6, 0.62), at(6, 0.7));
  // The message is complete; a beat to read it before the commit.
  press(tl, commitButton, at(6, 0.74), at(6, 0.78));
  tl.to(newCommit, { height: "auto", autoAlpha: 1, duration: span(6, 0.78, 0.84), ease: "power2.out" }, at(6, 0.78));
  flip(tl, treeLabel, at(6, 0.79), { text: "Working tree" });
  flip(tl, treeCount, at(6, 0.79), { text: "0" });
  tl.to(changedFiles, { autoAlpha: 0, height: 0, paddingTop: 0, paddingBottom: 0, duration: span(6, 0.79, 0.83), ease: "power2.in" }, at(6, 0.79));
  show(tl, cleanTree, at(6, 0.82), at(6, 0.85));
  tl.to(git.state, { lift: 0, duration: span(6, 0.87, 1), ease: "power3.inOut" }, at(6, 0.87));
  hide(tl, git.state, at(7, 0.78), at(7, 0.8));
}

// Act 7: the triage lifts out to reading size, narrows to what needs a
// person, opens the finding with its diff and evidence, holds, then a person
// approves and the tablet shows the merge.
function reviewBeats(tl, panels) {
  const review = panels.review;
  const root = review.element;
  const row = (name) => root.querySelector(`[data-triage="${name}"]`);
  const finding = root.querySelector("[data-finding]");
  const approval = root.querySelector("[data-approval]");
  const approveButton = root.querySelector("[data-approve-button]");
  const approved = root.querySelector("[data-approved]");
  const pill = root.querySelector("[data-review-pill]");
  gsap.set(row("rest"), { autoAlpha: 0, height: 0 });
  gsap.set(finding, { height: 0, autoAlpha: 0 });
  gsap.set(approval, { autoAlpha: 0 });
  gsap.set(approved, { autoAlpha: 0 });

  show(tl, review.state, at(7, 0.17), at(7, 0.25));
  tl.to(review.state, { lift: 1, duration: span(7, 0.27, 0.36), ease: "power3.inOut" }, at(7, 0.27));
  tl.to([row("verified"), row("failed")], { autoAlpha: 0, height: 0, marginTop: 0, duration: span(7, 0.37, 0.45), ease: "power2.inOut" }, at(7, 0.37));
  tl.to(row("rest"), { autoAlpha: 1, height: "auto", duration: span(7, 0.41, 0.48), ease: "power2.out" }, at(7, 0.41));
  tl.to(finding, { height: "auto", autoAlpha: 1, duration: span(7, 0.45, 0.55), ease: "power2.out" }, at(7, 0.45));
  // .55 to .66: the reading hold.
  tl.to(approval, { autoAlpha: 1, duration: span(7, 0.66, 0.69), ease: "power2.out" }, at(7, 0.66));
  press(tl, approveButton, at(7, 0.7), at(7, 0.73));
  tl.to(approveButton, { autoAlpha: 0, duration: span(7, 0.73, 0.75) }, at(7, 0.73));
  tl.to(approved, { autoAlpha: 1, duration: span(7, 0.74, 0.77), ease: "power2.out" }, at(7, 0.74));
  flip(tl, pill, at(7, 0.74), { text: "Approved", className: "done" });
  tl.to(review.state, { lift: 0, duration: span(7, 0.78, 0.82), ease: "power3.inOut" }, at(7, 0.78));
  hide(tl, review.state, at(7, 0.82), at(7, 0.85));
}

export function createOverlays({ film, stage, pose }) {
  const root = film.querySelector("[data-overlays]");
  if (!root) throw new Error("The film needs its overlays.");
  const panels = {};
  for (const element of root.querySelectorAll("[data-panel]")) panels[element.dataset.panel] = readPanel(element);
  for (const panel of Object.values(panels)) gsap.set(panel.element, { autoAlpha: 0 });

  function update() {
    const viewport = { width: film.clientWidth, height: film.clientHeight };
    for (const panel of Object.values(panels)) {
      const { element, state } = panel;
      if (state.visible <= 0.001) {
        element.style.visibility = "hidden";
        continue;
      }
      const corners = stage.screenCorners(panel.device, pose[panel.device]);
      const region = [state.x, state.y, panel.width, panel.height];
      const flat = panel.flat
        ? flatQuad(region, { cx: viewport.width * panel.flat.fx, cy: viewport.height * panel.flat.fy, width: viewport.width * panel.flat.fw })
        : null;
      element.style.transform = panelTransform({
        screenCorners: corners,
        textureSize: TEXTURE_SIZES[panel.device],
        region,
        lift: state.lift,
        flat,
      });
      element.style.opacity = String(state.visible);
      element.style.visibility = "visible";
      element.style.setProperty("--lift", String(state.lift));
    }
  }

  return {
    panels,
    addTo(tl) {
      editorBeats(tl, panels);
      issueBeats(tl, panels);
      teamBeats(tl, panels);
      builderBeats(tl, panels);
      gitBeats(tl, panels);
      reviewBeats(tl, panels);
    },
    update,
    dispose() {
      for (const panel of Object.values(panels)) {
        panel.element.style.transform = "";
        panel.element.style.visibility = "";
        panel.element.style.opacity = "";
      }
    },
  };
}
