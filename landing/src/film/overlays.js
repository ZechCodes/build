// The close-ups: HTML panels that sit on a device's screen, projected every
// frame from the stage's corners, and the beats that animate what is on them.
// A panel's visibility is the scroll timeline's, like a device move: showing
// a panel that matches the fixture under it is nothing to look at. What a
// person reads (a lift, typing, a card moving, a commit, an approval) is a
// scene: its own timeline in seconds, played once the act has arrived.
// update() turns each panel's state into a matrix3d.
import gsap from "gsap";
import { SCENES, TEXTURE_SIZES, at, sceneClock, span } from "./acts.js";
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
// class gained (or, with `off`, a class lost), or both. The text to restore
// is read as the change fires, so an element flipped twice hands back what
// the earlier flip left, not what the markup started with.
export function flip(tl, element, time, { text, className, off = false, onForward, onBackward, read } = {}) {
  let before = null;
  let applied = false;
  const apply = (forward) => {
    if (forward === applied) return;
    applied = forward;
    if (forward) before = read ? read() : element.textContent;
    if (onForward) {
      if (forward) onForward();
      else onBackward(before);
    } else if (text !== undefined) {
      element.textContent = forward ? text : before;
    }
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

// A highlight that comes up on an element and fades as it moves on.
function pulse(tl, element, from, until) {
  const rise = (until - from) * 0.4;
  tl.to(element, { "--pulse": 1, duration: rise, ease: "power2.out" }, from);
  tl.to(element, { "--pulse": 0, duration: (until - from) * 0.9, ease: "power2.in" }, from + rise);
}

// A display swap that belongs to a scene: set on the clock, undone by a
// rewind, and holding only until the timeline leaves the act.
function screenCue(tl, screens, actId, time, device, name) {
  const until = at(actId + 1, 0);
  flip(tl, { textContent: "" }, time, {
    text: name,
    onForward: () => screens.set(device, { name, until }),
    onBackward: (before) => (before ? screens.set(device, before) : screens.delete(device)),
    read: () => screens.get(device),
  });
}

// What the scroll timeline does to the close-ups: each shows as its act
// arrives, matching the fixture under it, and goes as the act leaves.
function visibilityBeats(tl, panels) {
  show(tl, panels.editor.state, at(1, 0.18), at(1, 0.21));
  hide(tl, [panels.editor.state, panels.status.state], at(2, 0.9), at(3, 0));
  show(tl, panels.issue.state, at(3, 0.18), at(3, 0.2));
  hide(tl, [panels.issue.state, panels.hole.state], at(3, 0.98), at(4, 0.01));
  show(tl, panels.team.state, at(4, 0), at(4, 0.01));
  hide(tl, panels.team.state, at(4, 0.96), at(5, 0));
  show(tl, panels.builder.state, at(5, 0.12), at(5, 0.15));
  hide(tl, panels.builder.state, at(6, 0), at(6, 0.03));
  show(tl, panels.git.state, at(6, 0.02), at(6, 0.05));
  // The laptop shrinks into its host pose for act 7; the Git close-up goes
  // with the move, before the merge can reach the display under it.
  hide(tl, panels.git.state, at(6, 0.86), at(7, 0));
  show(tl, panels.review.state, at(7, 0.17), at(7, 0.25));
  hide(tl, panels.review.state, at(7, 0.84), at(7, 0.87));
}

// Act 1: the laptop has finished its push; a test types in, the gutter marks
// both lines, the status bar ticks to "2 changes".
function editorBeats(tl, panels, { at, span }) {
  const editor = panels.editor;
  const status = panels.status;
  const lines = editor.element.querySelectorAll("[data-typed-line]");
  const typed = editor.element.querySelectorAll("[data-type]");
  gsap.set(lines, { "--mark": 0 });
  caret(tl, editor.element, "1", at(1, 0.21), at(1, 0.43));
  typeInto(tl, typed[0], at(1, 0.22), at(1, 0.43));
  tl.to(lines[0], { "--mark": 1, duration: span(1, 0, 0.02) }, at(1, 0.43));
  caret(tl, editor.element, "2", at(1, 0.44), at(1, 0.65));
  typeInto(tl, typed[1], at(1, 0.45), at(1, 0.63));
  typeInto(tl, typed[2], at(1, 0.63), at(1, 0.65));
  tl.to([lines[1], lines[2]], { "--mark": 1, duration: span(1, 0, 0.02) }, at(1, 0.65));
  show(tl, status.state, at(1, 0.66), at(1, 0.7));
}

// Act 2: the three captions arrive one at a time, then hold.
function captionBeats(tl, film, { at, span }) {
  const captions = film.querySelectorAll('[data-act="2"] .captions li');
  captions.forEach((caption, index) => {
    const start = [0.25, 0.45, 0.65][index] ?? 0.65;
    tl.fromTo(caption, { autoAlpha: 0, y: 16 }, { autoAlpha: 1, y: 0, duration: span(2, 0, 0.08), ease: "power2.out" }, at(2, start));
  });
}

// Act 3: the card lifts out of Ready, gets its assignee, moves to In progress
// with its branch, and is back in the board before the laptop moves on.
function issueBeats(tl, panels, { at, span }) {
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
  tl.to(issue.state, { lift: 1, duration: span(3, 0.2, 0.34), ease: "power3.inOut" }, at(3, 0.2));
  show(tl, hole.state, at(3, 0.2), at(3, 0.26));
  hide(tl, none, at(3, 0.35), at(3, 0.4));
  show(tl, agent, at(3, 0.38), at(3, 0.48));
  hide(tl, ready, at(3, 0.5), at(3, 0.55));
  show(tl, progress, at(3, 0.53), at(3, 0.6));
  tl.to(branch, { autoAlpha: 1, height: "auto", duration: span(3, 0.58, 0.68), ease: "power2.out" }, at(3, 0.58));
  tl.to(branch, { autoAlpha: 0, height: 0, duration: span(3, 0.72, 0.76), ease: "power2.in" }, at(3, 0.72));
  tl.to(issue.state, { lift: 0, x: 892, y: 362, duration: span(3, 0.74, 0.85), ease: "power3.inOut" }, at(3, 0.74));
}

// Act 4: two more agents join, Implement waits on a question the phone
// answers, and gets back to work.
function teamBeats(tl, panels, { at, span }, screens) {
  const team = panels.team;
  const rows = team.element.querySelectorAll("[data-team-row]");
  const count = team.element.querySelector("[data-running-count]");
  const status = team.element.querySelector('[data-row-status="implement"]');
  gsap.set([rows[1], rows[2]], { autoAlpha: 0, x: 40 });
  const counter = { value: 1 };
  tl.to(rows[1], { autoAlpha: 1, x: 0, duration: span(4, 0.22, 0.3), ease: "power3.out" }, at(4, 0.22));
  tl.to(rows[2], { autoAlpha: 1, x: 0, duration: span(4, 0.32, 0.4), ease: "power3.out" }, at(4, 0.32));
  tl.to(counter, { value: 3, duration: span(4, 0.22, 0.4), snap: "value", onUpdate: () => { count.textContent = String(counter.value); } }, at(4, 0.22));
  flip(tl, status, at(4, 0.5), { text: "Waiting", className: "waiting" });
  screenCue(tl, screens, 4, at(4, 0.64), "phone", "ui03-answer-iphone");
  screenCue(tl, screens, 4, at(4, 0.78), "phone", "ui03-resumed-iphone");
  flip(tl, status, at(4, 0.78), { text: "Working", className: "waiting", off: true });
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

// Act 5: the builder lifts, a Test step is dragged in between Implement and
// Review, the arrows redraw, and the handoff runs the new path as a pulse
// on each step's outline and the arrow after it, stopping at the gate.
function builderBeats(tl, panels, { at, span }) {
  const builder = panels.builder;
  const root = builder.element;
  const node = (name) => root.querySelector(`[data-node="${name}"]`);
  const arrow = (name) => root.querySelector(`[data-arrow="${name}"]`);
  const status = (name) => root.querySelector(`[data-node-status="${name}"]`);
  const caption = root.querySelector("[data-canvas-caption]");
  const layout = { implement: 0, review: 1, gate: 2, merge: 3 };
  for (const [name, index] of Object.entries(layout)) gsap.set(node(name), { x: FOUR_UP[index], y: NODE_TOP });
  gsap.set(node("test"), { ...PALETTE_TEST, autoAlpha: 0, scale: 0.6 });
  gsap.set(arrow("implement-review"), { x: arrowX(FOUR_UP[0], FOUR_UP[1]), y: nodeCentreY() });
  gsap.set(arrow("review-gate"), { x: arrowX(FOUR_UP[1], FOUR_UP[2]), y: nodeCentreY() });
  gsap.set(arrow("gate-merge"), { x: arrowX(FOUR_UP[2], FOUR_UP[3]), y: nodeCentreY() });
  gsap.set(arrow("implement-test"), { x: arrowX(FIVE_UP[0], FIVE_UP[1]), y: nodeCentreY(), autoAlpha: 0 });
  gsap.set(arrow("test-review"), { x: arrowX(FIVE_UP[1], FIVE_UP[2]), y: nodeCentreY(), autoAlpha: 0 });
  gsap.set(caption, { autoAlpha: 0, y: 10 });

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

  // The handoff: the highlight runs step, arrow, step along the new path. A
  // node it has left is finished, and finished is quiet.
  const path = [node("implement"), arrow("implement-test"), node("test"), arrow("test-review"), node("review"), arrow("review-gate"), node("gate")];
  const step = 0.03;
  path.forEach((element, index) => pulse(tl, element, at(5, 0.58 + step * index), at(5, 0.58 + step * (index + 1))));
  flip(tl, node("implement"), at(5, 0.61), { className: "settled" });
  flip(tl, status("implement"), at(5, 0.61), { text: "Done" });
  flip(tl, status("test"), at(5, 0.64), { text: "Passed ✓" });
  flip(tl, node("test"), at(5, 0.64), { className: "passed" });
  flip(tl, node("test"), at(5, 0.67), { className: "settled" });
  flip(tl, status("review"), at(5, 0.7), { text: "Done" });
  flip(tl, node("review"), at(5, 0.7), { className: "passed" });
  flip(tl, node("review"), at(5, 0.73), { className: "settled" });
  flip(tl, status("gate"), at(5, 0.76), { text: "Waiting for you" });
  flip(tl, node("gate"), at(5, 0.76), { className: "waiting" });
  tl.to(caption, { autoAlpha: 1, y: 0, duration: span(5, 0.8, 0.85), ease: "power2.out" }, at(5, 0.8));
  tl.to(builder.state, { lift: 0, duration: span(5, 0.88, 1), ease: "power3.inOut" }, at(5, 0.88));
}

// Act 6: gutter highlights, author chips, one line added to the diff (the
// hunk's count goes +3 to +4, as the document's proof says), stage, a hold
// on the finished message, commit; the graph gains the visitor's commit and
// the working tree is clean.
function gitBeats(tl, panels, { at, span }) {
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

  tl.to(git.state, { lift: 1, duration: span(6, 0.1, 0.2), ease: "power3.inOut" }, at(6, 0.1));
  tl.to(hunkAgent, { "--lit": 1, duration: span(6, 0.2, 0.26) }, at(6, 0.2));
  tl.to(chipAgent, { autoAlpha: 1, x: 0, duration: span(6, 0.22, 0.28), ease: "power2.out" }, at(6, 0.22));
  tl.to(hunkYou, { "--lit": 1, duration: span(6, 0.3, 0.36) }, at(6, 0.3));
  tl.to(chipYou, { autoAlpha: 1, x: 0, duration: span(6, 0.32, 0.38), ease: "power2.out" }, at(6, 0.32));
  tl.to(humanLine, { height: "auto", autoAlpha: 1, duration: span(6, 0.4, 0.43), ease: "power2.out" }, at(6, 0.4));
  caret(tl, root, "human", at(6, 0.4), at(6, 0.55));
  typeInto(tl, humanText, at(6, 0.42), at(6, 0.54));
  flip(tl, diffAdd, at(6, 0.54), { text: "+4" });
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
}

// Act 7: the triage lifts out to reading size, narrows to what needs a
// person, opens the finding with its diff and evidence, holds, then a person
// approves and the displays show the merge.
function reviewBeats(tl, panels, { at, span }, screens) {
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

  tl.to(review.state, { lift: 1, duration: span(7, 0.27, 0.36), ease: "power3.inOut" }, at(7, 0.27));
  tl.to([row("verified"), row("failed")], { autoAlpha: 0, height: 0, marginTop: 0, duration: span(7, 0.37, 0.45), ease: "power2.inOut" }, at(7, 0.37));
  tl.to(row("rest"), { autoAlpha: 1, height: "auto", duration: span(7, 0.41, 0.48), ease: "power2.out" }, at(7, 0.41));
  tl.to(finding, { height: "auto", autoAlpha: 1, duration: span(7, 0.45, 0.55), ease: "power2.out" }, at(7, 0.45));
  // .55 to .66: the reading hold.
  screenCue(tl, screens, 7, at(7, 0.65), "tablet", "ui05-approval-ipad");
  tl.to(approval, { autoAlpha: 1, duration: span(7, 0.66, 0.69), ease: "power2.out" }, at(7, 0.66));
  press(tl, approveButton, at(7, 0.7), at(7, 0.73));
  tl.to(approveButton, { autoAlpha: 0, duration: span(7, 0.73, 0.75) }, at(7, 0.73));
  tl.to(approved, { autoAlpha: 1, duration: span(7, 0.74, 0.77), ease: "power2.out" }, at(7, 0.74));
  flip(tl, pill, at(7, 0.74), { text: "Approved", className: "done" });
  tl.to(review.state, { lift: 0, duration: span(7, 0.78, 0.82), ease: "power3.inOut" }, at(7, 0.78));
  screenCue(tl, screens, 7, at(7, 0.8), "laptop", "ui05-merged-macbook");
  screenCue(tl, screens, 7, at(7, 0.82), "tablet", "ui05-merged-ipad");
}

const SCENE_BEATS = {
  1: (tl, panels, clock) => editorBeats(tl, panels, clock),
  2: (tl, panels, clock, screens, film) => captionBeats(tl, film, clock),
  3: (tl, panels, clock) => issueBeats(tl, panels, clock),
  4: (tl, panels, clock, screens) => teamBeats(tl, panels, clock, screens),
  5: (tl, panels, clock) => builderBeats(tl, panels, clock),
  6: (tl, panels, clock) => gitBeats(tl, panels, clock),
  7: (tl, panels, clock, screens) => reviewBeats(tl, panels, clock, screens),
};

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
      visibilityBeats(tl, panels);
    },
    // One paused timeline per act with a scene, in seconds; the film plays
    // it when the act arrives. `screens` receives the scene's display swaps.
    scenes(screens) {
      const scenes = {};
      for (const actId of Object.keys(SCENES).map(Number)) {
        const tl = gsap.timeline({ paused: true, defaults: { ease: "none" } });
        SCENE_BEATS[actId](tl, panels, sceneClock(actId), screens, film);
        scenes[actId] = tl;
      }
      return scenes;
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
