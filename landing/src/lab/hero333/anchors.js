// Where the hero's three requests land: their rows on the laptop's screen,
// in the screen texture's own pixels (1512×982, TEXTURE_SIZES.laptop). The
// numbers are the Tasks dashboard rows design/landing-captures/capture.mjs
// recorded for this texture; test/hero/anchors.test.mjs holds them to the
// capture.
export const HERO_SCREEN = "ui09-needs-you-macbook";
export const HERO_ROW_REGIONS = Object.freeze({
  "task-82": Object.freeze([375, 320.73, 1004, 59.64]),
  "task-85": Object.freeze([375, 380.38, 1004, 60.64]),
  "task-86": Object.freeze([375, 441.02, 1004, 60.64]),
});
// Where on its row a request lands: a little in from the row's left, over
// the task's number and title.
export const LANDING_POINT = Object.freeze([0.22, 0.5]);
