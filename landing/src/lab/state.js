// The lab page's state, kept in its query so a link sent back opens on
// exactly what its sender was looking at: the variant, the speed, whether
// the beat loops or the flood runs endlessly, and, when held, the moment.

export const SPEEDS = Object.freeze([0.25, 0.5, 1]);
export const DEFAULT_STATE = Object.freeze({ variant: "a", speed: 1, loop: true, endless: false, playing: true, time: 0 });

const flag = (value, fallback) => (value === "1" ? true : value === "0" ? false : fallback);

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
  const variant = query.get("v");
  const playing = query.get("paused") !== "1";
  return {
    variant: variants.includes(variant) ? variant : DEFAULT_STATE.variant,
    speed: readSpeed(query.get("speed")),
    loop: flag(query.get("loop"), DEFAULT_STATE.loop),
    endless: flag(query.get("endless"), DEFAULT_STATE.endless),
    playing,
    time: playing ? 0 : readTime(query.get("t")),
  };
}

export function writeState({ variant, speed, loop, endless, playing, time }) {
  const query = new URLSearchParams({ v: variant, speed: String(speed), loop: loop ? "1" : "0", endless: endless ? "1" : "0" });
  if (!playing) {
    query.set("paused", "1");
    query.set("t", String(Math.round(time * 100) / 100));
  }
  return `?${query}`;
}
