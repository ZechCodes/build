// The hero's laptop during the entrance: one reveal, drawn one of two ways.
// The stage draws it when the film's 3D is ready before the reveal starts;
// otherwise the captured picture plays the same arc (the phone always does).
// The choice is made once, on the reveal's first frame, and kept: the film
// hands over from the picture to the stage only after the hero is still.
//
// Everything is in the hero's own pixels, from its top left corner: the film
// pins the hero to the window, so there the stage's pixels are the hero's.
import { TEXTURE_SIZES } from "../../film/acts.js";
import { homographyFromQuad, regionQuad, screenQuadFromCorners } from "../../stage/overlay.js";
import { HERO_ROW_REGIONS } from "./anchors.js";
import { HERO_POSTER } from "./poster-anchors.js";
import { laptopEntrancePose, laptopRevealPose, posterReveal } from "./timing.js";

function stageQuads(stage, pose) {
  const screen = screenQuadFromCorners(stage.corners(pose));
  const [width, height] = TEXTURE_SIZES.laptop;
  const homography = homographyFromQuad(width, height, screen);
  const rows = {};
  for (const [row, region] of Object.entries(HERO_ROW_REGIONS)) rows[row] = regionQuad(homography, region);
  return { screen, rows };
}

// The picture's quads, from its box before any transform: the frame inside
// it is what turns, the box stays where the picture rests.
function posterQuads(hero, device) {
  const heroBox = hero.getBoundingClientRect();
  const box = device.getBoundingClientRect();
  const place = ([fx, fy]) => [box.left - heroBox.left + fx * box.width, box.top - heroBox.top + fy * box.height];
  const rows = {};
  for (const [row, quad] of Object.entries(HERO_POSTER.rows)) rows[row] = quad.map(place);
  return { screen: HERO_POSTER.screen.map(place), rows };
}

export function createHeroLaptop({ hero, device, frame, narrow }) {
  const reveal = { t: 0 };
  // Set by the film: the pose object its frames draw, the hero's resting
  // pose, and the stage's corner projection.
  let stage = null;
  let from = null;
  let driver = null;
  let pictureQuads = null;

  function drawPicture(t) {
    const { turn, shift, scale, opacity } = posterReveal(t, { narrow });
    frame.style.transform = t >= 1 ? "" : `translateX(${shift}vw) rotateY(${-turn}deg) scale(${scale})`;
    device.style.opacity = String(opacity);
    device.style.visibility = opacity > 0 ? "visible" : "hidden";
  }

  function hidePicture() {
    device.style.opacity = "0";
    device.style.visibility = "hidden";
  }

  function render() {
    if (!driver && reveal.t > 0) driver = stage ? "stage" : "poster";
    // Before the reveal a waiting stage holds the laptop out of sight too.
    if (driver === "stage" || (!driver && stage)) Object.assign(stage.pose, laptopRevealPose(reveal.t, from, stage.final));
    else drawPicture(reveal.t);
  }

  return {
    reveal,
    render,
    get driver() { return driver; },
    /** The screen and each Needs you row as quads in the hero's pixels,
     *  where the laptop is now, or where it rests with `resting`. */
    quads({ resting = false } = {}) {
      if (stage && driver !== "poster") return stageQuads(stage, resting || !driver ? stage.final : stage.pose);
      pictureQuads ||= posterQuads(hero, device);
      return pictureQuads;
    },
    /** The film's stage is ready. It draws the reveal if the reveal has not
     *  begun; otherwise the picture finishes it and the film hands over. */
    attachStage(api) {
      if (driver === "poster") return "handover";
      stage = api;
      from = laptopEntrancePose(api.final, { narrow });
      hidePicture();
      render();
      return "drive";
    },
    /** The laptop at rest. The picture is left in place for the document and
     *  the film's hand-over; a stage that drew the reveal keeps the stage. */
    settle() {
      reveal.t = 1;
      render();
      if (driver === "stage") {
        hidePicture();
        return;
      }
      frame.style.transform = "";
      device.style.opacity = "";
      device.style.visibility = "";
    },
  };
}
