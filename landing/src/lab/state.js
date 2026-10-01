// The lab page's state, kept in its query so a link sent back opens on
// exactly what its sender was looking at: the variant, the speed, whether
// the beat loops or the flood runs endlessly, the wall's shape and how it
// blurs (#316), and, when held, the moment.

export const SPEEDS = Object.freeze([0.25, 0.5, 1]);
export const SHAPES = Object.freeze(["dome", "full"]);
// The wall's blur-out: a cross-fade to a copy drawn blurred once, or an
// animated filter.
export const BLURS = Object.freeze(["copy", "filter"]);
export const DEFAULT_STATE = Object.freeze({ variant: "e", speed: 1, loop: true, endless: false, shape: "dome", blur: "copy", playing: true, time: 0 });

const flag = (value, fallback) => (value === "1" ? true : value === "0" ? false : fallback);
const oneOf = (value, choices, fallback) => (choices.includes(value) ? value : fallback);

function readSpeed(value) {
  const speed = Number(value);
  return SPEEDS.includes(speed) ? speed : DEFAULT_STATE.speed;
}

function readTime(value) {
  const time = Number(value);
  return Number.isFinite(time) && time >= 0 ? time : 0;
}

/** The state a query names; anything unknown falls back to the default. A
 *  link to a running field starts it from the beginning. */
export function readState(search, variants) {
  const query = new URLSearchParams(search);
  const playing = query.get("paused") !== "1";
  return {
    variant: oneOf(query.get("v"), variants, DEFAULT_STATE.variant),
    speed: readSpeed(query.get("speed")),
    loop: flag(query.get("loop"), DEFAULT_STATE.loop),
    endless: flag(query.get("endless"), DEFAULT_STATE.endless),
    shape: oneOf(query.get("shape"), SHAPES, DEFAULT_STATE.shape),
    blur: oneOf(query.get("blur"), BLURS, DEFAULT_STATE.blur),
    playing,
    time: playing ? 0 : readTime(query.get("t")),
  };
}

export function writeState({ variant, speed, loop, endless, shape, blur, playing, time }) {
  const query = new URLSearchParams({ v: variant, speed: String(speed), loop: loop ? "1" : "0", endless: endless ? "1" : "0", shape, blur });
  if (!playing) {
    query.set("paused", "1");
    query.set("t", String(Math.round(time * 100) / 100));
  }
  return `?${query}`;
}
