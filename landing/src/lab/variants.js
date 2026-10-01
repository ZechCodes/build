// The variants the lab can show, behind its switch. Each is a flood (the
// beat, ending where Build takes over) and an endless form that never
// resolves; both take the measured field and the stage and answer an
// update(time, playing, rate) and their end.
import { liveEndless, liveFlood } from "./live.js";

export const VARIANTS = Object.freeze({
  a: Object.freeze({ label: "A", title: "A (live)", flood: liveFlood, endless: liveEndless }),
});

export const VARIANT_IDS = Object.freeze(Object.keys(VARIANTS));
