import { createWall, HARNESS_NAMES, notePose, WALL_TIMING } from './wall.js';
import { seekAll } from './seeker.js';

function element(document, tag, className, text = '') {
  const node = document.createElement(tag);
  node.className = className;
  node.textContent = text;
  return node;
}

function card(document, note, className) {
  const node = element(document, 'div', className);
  node.dataset.harness = note.harness;
  const header = element(document, 'span', 'wall-note__header');
  header.append(element(document, 'i', 'wall-note__mark'), element(document, 'span', 'wall-note__agent', HARNESS_NAMES[note.harness]), element(document, 'span', 'wall-note__time', 'now'));
  node.append(header, element(document, 'span', 'wall-note__text', note.text));
  return node;
}

function place(node, position) {
  node.style.left = `${position.x - position.width / 2}px`;
  node.style.top = `${position.y - position.height / 2}px`;
  node.style.width = `${position.width}px`;
  node.style.height = `${position.height}px`;
}

function frames(turn) {
  const end = WALL_TIMING.ripple[1];
  const times = [0, end, turn.start + turn.enter, turn.start + turn.enter + turn.hold];
  for (let step = 0; step <= 10; step += 1) times.push(turn.start + turn.enter * step / 10);
  for (let step = 0; step <= 6; step += 1) times.push(turn.start + turn.enter + turn.hold + turn.fade * step / 6);
  return [...new Set(times.filter(time => time >= 0 && time <= end))].sort((a, b) => a - b).map(time => {
    const pose = notePose(turn, time);
    return { offset: time / end, transform: `translate3d(${pose.x}px,${pose.y}px,0)`, opacity: pose.opacity };
  });
}

function selectedCard(document, note, position, slot, request, field) {
  note.classList.add('wall-request');
  note.dataset.attention = request.turn.attention;
  const green = card(document, request.turn, 'wall-note wall-note--mint');
  green.querySelector('.wall-note__time').textContent = 'needs you';
  note.append(green);
  const layer = flying => {
    const parent = flying ? field : position;
    if (note.parentElement === parent) return;
    parent.append(note);
    place(note, flying ? slot : { ...slot, x: slot.width / 2, y: slot.height / 2 });
  };
  return { element: note, green, position, layer, ...request };
}

export function createWallMotion(field) {
  const { width, height } = field.getBoundingClientRect();
  const document = field.ownerDocument;
  const layout = createWall({ width, height });
  const routine = field.querySelector('[data-wall-routine]');
  // Swap the static first paint for the measured, independently timed wall.
  routine.replaceChildren();
  const timed = [];
  const requests = [];
  layout.slots.forEach((slot, index) => {
    const position = element(document, 'div', 'wall-slot');
    position.dataset.wallSlot = String(index);
    position.dataset.depth = slot.depth;
    place(position, slot);
    slot.turns.forEach((turn, turnIndex) => {
      const note = card(document, turn, 'wall-note');
      note.dataset.note = `${index}-${turnIndex}`;
      note.dataset.slot = String(index);
      position.append(note);
      if (turn.attention) {
        const requestIndex = layout.requests.findIndex(request => request.slotIndex === index);
        requests[requestIndex] = selectedCard(document, note, position, slot, layout.requests[requestIndex], field);
        return;
      }
      const animation = note.animate(frames(turn), { duration: WALL_TIMING.ripple[1] * 1000, fill: 'both', easing: 'linear' });
      animation.pause();
      timed.push([animation, WALL_TIMING.ripple[1] * 1000]);
    });
    routine.append(position);
  });
  return { requests, update: seekAll(timed), dispose: () => timed.forEach(([animation]) => animation.cancel()) };
}
