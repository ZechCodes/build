// The notification field's motion through the entrance, on the compositor.
// A field this full cannot be moved by writing styles every frame: a phone
// spends whole frames restyling the pills. So every lane drifts on one Web
// Animation, and each pill, when the wave reaches it, brakes (and, unless
// it needs a person, fades) on one of its own that lives only while it
// moves. The entrance's clock calls update(): while it plays, the
// animations run on the compositor's time and are only caught up if they
// fall behind; while it is held or scrubbed, they are paused and seeked to
// it. Between those, a pill is styled once, when its part changes.
//
// The paths themselves are timing.js's; nothing here reads layout.
import { pillPath, reachesField } from "./timing.js";

// How far an animation may drift from the clock before it is caught up, ms.
const SLACK = 50;
// Points sampled along a pill's brake; the animation runs straight between
// them.
const STEPS = 8;

const px = (value) => `${Math.round(value * 10) / 10}px`;
const translate = ([x, y]) => `translate(${px(x)}, ${px(y)})`;

// A lane drifts on until the ripple has passed, then holds: every pill it
// carries has faded or stopped by then.
const laneShift = (lane, time, { timing, start }) => lane.speed * (Math.min(time, timing.ripple[1]) - start);

// Past its end an animation is held there, paused: play() on one at its end
// would rewind it to the start.
function seek(animation, ms, playing, duration) {
  if (!playing || ms >= duration) {
    animation.pause();
    animation.currentTime = Math.min(ms, duration);
  } else if (animation.playState !== "running") {
    animation.currentTime = ms;
    animation.play();
  } else if (Math.abs(animation.currentTime - ms) > SLACK) {
    animation.currentTime = ms;
  }
}

function setStyle(element, [transform, opacity]) {
  element.style.transform = transform;
  element.style.opacity = opacity;
  element.style.visibility = opacity === "0" ? "hidden" : "";
}

function laneMotion(lane, ripple) {
  const end = ripple.timing.ripple[1];
  const animation = lane.element.animate([
    { offset: 0, transform: `translateX(${px(lane.x + laneShift(lane, 0, ripple))})` },
    { offset: 1, transform: `translateX(${px(lane.x + laneShift(lane, end, ripple))})` },
  ], { duration: end * 1000, fill: "both", easing: "linear" });
  return (time, playing) => seek(animation, time * 1000, playing, end * 1000);
}

// One pill's part: still with its lane before `from`, animated until `to`,
// then held in its final style.
function pillMotion(pill, { from, to, frame, final }) {
  let part = null;
  let animation = null;
  const keyframes = () => Array.from({ length: STEPS + 1 }, (_, step) => {
    const offset = step / STEPS;
    return { offset, ...frame(from + (to - from) * offset) };
  });
  return (time, playing) => {
    const now = time < from ? "before" : time < to ? "moving" : "after";
    if (now === "moving") {
      animation ??= pill.element.animate(keyframes(), { duration: (to - from) * 1000, fill: "both", easing: "linear" });
      seek(animation, (time - from) * 1000, playing, (to - from) * 1000);
    } else if (animation) {
      animation.cancel();
      animation = null;
    }
    if (now !== part && now !== "moving") setStyle(pill.element, now === "before" ? ["", ""] : final());
    part = now;
  };
}

/** Moves the measured field (entrance.js) along its ripple. `flights` is
 *  when each request takes off for its row; its flight moves it from
 *  there, from offsetAt(pill, takeOff). */
export function createFieldMotion({ field, ripple, flights }) {
  const lanes = field.lanes.filter((lane) => lane.shown);
  const until = ripple.timing.ripple[1] - ripple.start;
  // Each pill in play, by its element: its lane and its way through.
  const placed = new Map();
  const offsetAt = (pill, time) => {
    const { lane, path } = placed.get(pill.element);
    const { dx, dy } = path.at(time);
    return [dx - laneShift(lane, time, ripple), dy];
  };
  const routinePart = (pill, path) => ({
    from: path.hit,
    to: path.hit + ripple.timing.fade,
    frame: (time) => ({ transform: translate(offsetAt(pill, time)), opacity: String(Math.round(pill.opacity * (1 - path.at(time).faded) * 1000) / 1000) }),
    final: () => ["", "0"],
  });
  const requestPart = (pill, path) => {
    const takeOff = flights.get(pill.attention);
    return { from: path.hit, to: takeOff, frame: (time) => ({ transform: translate(offsetAt(pill, time)) }), final: () => [translate(offsetAt(pill, takeOff)), ""] };
  };
  const movers = lanes.map((lane) => laneMotion(lane, ripple));
  for (const lane of lanes) {
    for (const pill of lane.pills.filter((candidate) => candidate.shown && reachesField(candidate, lane.speed, field.width, until))) {
      const path = pillPath(pill, lane.speed, ripple);
      placed.set(pill.element, { lane, path });
      movers.push(pillMotion(pill, (pill.attention ? requestPart : routinePart)(pill, path)));
    }
  }
  return {
    update(time, playing) {
      for (const move of movers) move(time, playing);
    },
    pathOf: (pill) => placed.get(pill.element)?.path,
    offsetAt,
  };
}
