// The notification field measured where it stands, for flood.js to move.
import { DRIFT_SECONDS } from "./field.js";

// The field where the CSS drift has got it: each lane's offset, how long it
// has been moving, and every pill's centre in the hero's pixels. The one
// read of layout the entrance (and the lab page) makes; nothing reads it per
// frame.
export function measureField(hero, field) {
  const heroBox = hero.getBoundingClientRect();
  const lanes = [...field.querySelectorAll(".hero-lane")].map((element) => {
    // A lane a phone leaves out has nothing to measure.
    if (!element.getClientRects().length) return { element, x: 0, speed: 0, pills: [], shown: false };
    const style = getComputedStyle(element);
    const x = new DOMMatrixReadOnly(style.transform).m41;
    // The drift is in vw, and a vw is a hundredth of the window; a lane
    // running left has a negative one, and a phone's run faster (hero.css).
    const scale = Number(style.getPropertyValue("--drift-scale")) || 1;
    const speed = (Number(element.dataset.drift) * scale / 100) * innerWidth / DRIFT_SECONDS;
    const pills = [...element.querySelectorAll(".hero-pill")].map((pill) => {
      const box = pill.getBoundingClientRect();
      return {
        element: pill,
        attention: pill.dataset.attention || null,
        x: box.left - heroBox.left + box.width / 2,
        y: box.top - heroBox.top + box.height / 2,
        width: box.width,
        shown: box.width > 0,
        opacity: Number(getComputedStyle(pill).opacity),
      };
    });
    return { element, x, speed, pills, shown: pills.some((pill) => pill.shown) };
  });
  const moving = lanes.find((lane) => lane.shown && lane.speed !== 0);
  return { width: heroBox.width, height: heroBox.height, lanes, elapsed: moving ? moving.x / moving.speed : 0 };
}
