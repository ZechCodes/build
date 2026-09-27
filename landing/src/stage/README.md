# Device stage

Three GLB devices on a studio-lit perspective stage. No scenes, no scroll, no
DOM but the canvas: the choreography drives it from GSAP timelines.

```js
const stage = createDeviceStage({ canvas, assetBase: "/landing/assets/devices", screenBase: "/landing/assets/screens" });
await stage.load(["laptop", "phone"]);                   // laptop-low.glb, phone.glb, tablet.glb
await stage.setScreen("laptop", "ui10-editor-macbook");  // <screenBase>/<name>.webp
stage.preloadScreen("laptop", "ui12-tasks-macbook");    // decode ahead of the beat
stage.setPose("laptop", { x: 65, y: 57, w: 52, pitch: 4, lidOpen: 1 });
stage.show("phone"); stage.hide("tablet");
stage.resize(); stage.render(); stage.dispose();
```
Also `measureFrameCost({ frames, warmup })` (median ms/frame; compare with
`STAGE_LIMITS.slowFrameMs` in `fallback.js` and fall back to posters),
`setQualityScale(renderQualityScale(cost))`, `releaseUnusedScreens()`,
`isSoftwareRenderer()`, `screenCorners(device, pose?)`, `getState()`.
`setScreen` keeps the current display until the next has decoded, so a screen
never blinks empty; a superseded load never reaches the glass.

## Pose vocabulary (`pose.js`, pure maths, no three)

`{ x, y, w, yaw, pitch, roll, opacity, lidOpen, faceCamera }`. `x`/`y` are
percent of the stage, `w` is percent of stage width across the whole enclosure,
angles are degrees, `opacity` 0..1 (a fading device still renders solid, on its
own depth pass), `lidOpen` 0..1, `faceCamera` 0..1 turns the device toward the
lens (1 by default; 0 whenever HTML has to align).
`interpolatePose(from, to, amount)` is linear — the timeline owns the easing.

## Aligning HTML to a screen, and the hinge

```js
const w = poseWidthForScreenWidth("laptop", 44); // display covers 44% of stage width
const [bottomLeft, bottomRight, topRight, topLeft] =
  screenCornersPx("laptop", { x: 50, y: 56, w, faceCamera: 0, lidOpen: 1 }, viewport);
```
Four CSS-pixel corners of that pose's display plane, in the exported order;
drive the overlay's `matrix3d` from them. The laptop's plane is the lid at its
open angle, 15° off vertical, not the base. `lidOpen` 0 → 1 maps to hinge
rotation 90° → −15° (105° of travel) and `laptopLidAngleDegrees` reads it as an
angle off the deck (0.7 is 73.5°, the hero entrance start); exported corners are
baked at full open and swing about `hinge_pivot_m` for any other value.
