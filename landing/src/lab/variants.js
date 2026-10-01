// The variants the lab can show, behind its switch. Each is a flood (the
// beat, ending where Build takes over) and an endless form that never
// resolves; both take the measured field and the stage and answer an
// update(time, playing, rate) and their end.
import { burstFlood, crescendoFlood, shockwaveFlood } from "./chaos.js";
import { liveEndless, liveFlood } from "./live.js";

export const VARIANTS = Object.freeze({
  a: Object.freeze({ label: "A live", title: "A (live)", flood: liveFlood, endless: liveEndless }),
  b: Object.freeze({ label: "B burst", title: "B (burst)", flood: burstFlood, endless: (context) => burstFlood(context, { endless: true }) }),
  c: Object.freeze({ label: "C shockwave", title: "C (shockwave)", flood: shockwaveFlood, endless: (context) => shockwaveFlood(context, { endless: true }) }),
  d: Object.freeze({ label: "D crescendo", title: "D (crescendo)", flood: crescendoFlood, endless: (context) => crescendoFlood(context, { endless: true }) }),
});

export const VARIANT_IDS = Object.freeze(Object.keys(VARIANTS));
