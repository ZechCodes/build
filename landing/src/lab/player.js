// The lab's field on a chosen variant: puts the field back as HeroField drew
// it, measures it as the entrance does, and hands it to the variant.
import { measureField } from "../hero/measure.js";
import { NARROW_QUERY, stageFor } from "./stage.js";
import { VARIANTS } from "./variants.js";

const MOVED = ".hero-lane, .hero-pill";

// Everything a variant set, gone: its animations (the CSS sway stays) and
// its inline styles (the lanes' own custom properties stay).
function reset(field) {
  for (const animation of field.getAnimations({ subtree: true })) {
    if (!(animation instanceof CSSAnimation)) animation.cancel();
  }
  for (const element of field.querySelectorAll(MOVED)) {
    for (const property of ["transform", "opacity", "visibility", "z-index"]) element.style.removeProperty(property);
  }
}

export function createPlayer({ container, field }) {
  let flood = null;
  let last = null;
  return {
    /** The field on `variant`, endless or not. Answers the beat's end and
     *  the clock its phases are named on. */
    build({ variant, endless }) {
      reset(field);
      field.dataset.labVariant = variant;
      const measured = measureField(container, field);
      const stage = stageFor(matchMedia(NARROW_QUERY).matches, measured);
      const chosen = VARIANTS[variant];
      flood = (endless ? chosen.endless : chosen.flood)({ field: measured, stage });
      last = null;
      return { end: flood.end, timing: stage.timing };
    },
    // Only when something moved: a held field is not seeked every frame.
    update(time, playing, rate) {
      const now = `${time}|${playing}|${rate}`;
      if (now === last) return;
      last = now;
      flood.update(time, playing, rate);
    },
  };
}
