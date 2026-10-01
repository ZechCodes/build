// Capture the hero's laptop picture: the live stage at the hero's resting
// pose, lid open, on the Needs you screen, on a transparent background,
// cropped to the hardware. The page shows it until the stage's first frame,
// the entrance turns it in when the stage is not ready, and the document
// (phones, reduced motion) keeps it. Writes the picture, the placement
// numbers hero.css reads and the screen and row corners the entrance lands
// on (landing/src/hero/poster-anchors.js), so the three cannot drift apart.
// Start scripts/preview-landing.py on a fresh `npm run build` first.
//
//   LANDING_URL    where the preview listens (default http://127.0.0.1:4173)
//   CHROMIUM_PATH  a system Chromium instead of Playwright's download
import fs from "node:fs/promises";
import { chromium } from "playwright";
import { POSES, TEXTURE_SIZES } from "../landing/src/film/acts.js";
import { homographyFromQuad, regionQuad, screenQuadFromCorners } from "../landing/src/stage/overlay.js";
import { HERO_ROW_REGIONS } from "../landing/src/hero/anchors.js";

const base = process.env.LANDING_URL || "http://127.0.0.1:4173";
const WIDTH = 1440;
const HEIGHT = 900;
const SCALE = 2;
const image = new URL("../skriftapp/buildapp/landing/assets/devices/hero-needs-you.webp", import.meta.url);
const css = new URL("../landing/src/styles/hero.css", import.meta.url);
const anchors = new URL("../landing/src/hero/poster-anchors.js", import.meta.url);

const browser = await chromium.launch({
  headless: true,
  executablePath: process.env.CHROMIUM_PATH,
  args: ["--headless=new", "--use-gl=angle", "--use-angle=gl", "--ignore-gpu-blocklist"],
});

// The hardware's bounding box in the screenshot, a few pixels loose.
async function cropToHardware(page, shot) {
  return page.evaluate(async (dataUrl) => {
    const picture = new Image();
    picture.src = dataUrl;
    await picture.decode();
    const canvas = document.createElement("canvas");
    canvas.width = picture.width;
    canvas.height = picture.height;
    const context = canvas.getContext("2d");
    context.drawImage(picture, 0, 0);
    const { data } = context.getImageData(0, 0, canvas.width, canvas.height);
    let left = canvas.width, top = canvas.height, right = -1, bottom = -1;
    for (let y = 0; y < canvas.height; y += 1) {
      for (let x = 0; x < canvas.width; x += 1) {
        if (data[(y * canvas.width + x) * 4 + 3] < 4) continue;
        left = Math.min(left, x); right = Math.max(right, x);
        top = Math.min(top, y); bottom = Math.max(bottom, y);
      }
    }
    const pad = 4;
    left = Math.max(0, left - pad); top = Math.max(0, top - pad);
    right = Math.min(canvas.width - 1, right + pad); bottom = Math.min(canvas.height - 1, bottom + pad);
    const out = document.createElement("canvas");
    out.width = right - left + 1;
    out.height = bottom - top + 1;
    out.getContext("2d").drawImage(canvas, left, top, out.width, out.height, 0, 0, out.width, out.height);
    return { left, top, width: out.width, height: out.height, webp: out.toDataURL("image/webp", 0.9) };
  }, `data:image/png;base64,${shot.toString("base64")}`);
}

// The screen's and each row's corners in CSS pixels, from the stage's own
// projection of the resting pose and the same homography the close-ups use.
async function screenQuads(page) {
  const corners = await page.evaluate(() => window.BuildFilm.stage.screenCorners("laptop", window.BuildFilm.pose.laptop));
  const screen = screenQuadFromCorners(corners);
  const homography = homographyFromQuad(...TEXTURE_SIZES.laptop, screen);
  const rows = {};
  for (const [row, region] of Object.entries(HERO_ROW_REGIONS)) rows[row] = regionQuad(homography, region);
  return { screen, rows };
}

function anchorsModule(crop, quads) {
  const fraction = ([x, y]) => [Number(((x * SCALE - crop.left) / crop.width).toFixed(5)), Number(((y * SCALE - crop.top) / crop.height).toFixed(5))];
  const quad = (corners) => JSON.stringify(corners.map(fraction));
  const rows = Object.entries(quads.rows).map(([row, corners]) => `    "${row}": ${quad(corners)},`).join("\n");
  return `// Written by web/capture-hero-laptop.mjs with hero-needs-you.webp; do not
// edit. The picture's size in pixels, and the screen's and each Needs you
// row's corners [top left, top right, bottom right, bottom left] as
// fractions of the picture.
export const HERO_POSTER = {
  width: ${crop.width},
  height: ${crop.height},
  screen: ${quad(quads.screen)},
  rows: {
${rows}
  },
};
`;
}

async function writePlacement(crop) {
  // The stage sizes the laptop by the width and centres it at x% of the
  // width and y% of the height; everything else about the picture scales
  // with the width too.
  const hero = POSES.laptop[1];
  const vw = (pixels) => ((pixels / SCALE / WIDTH) * 100).toFixed(2);
  const anchorY = (hero.y / 100) * HEIGHT * SCALE;
  const rules = {
    "--cut-left": `${vw(crop.left)}vw`,
    "--cut-top": `calc(${hero.y}vh ${crop.top >= anchorY ? "+" : "-"} ${vw(Math.abs(crop.top - anchorY))}vw)`,
    "--cut-width": `${vw(crop.width)}vw`,
  };
  let sheet = await fs.readFile(css, "utf8");
  for (const [name, value] of Object.entries(rules)) {
    const pattern = new RegExp(`(${name}: )[^;]+;`);
    if (!pattern.test(sheet)) throw new Error(`hero.css has no ${name}`);
    sheet = sheet.replace(pattern, `$1${value};`);
  }
  await fs.writeFile(css, sheet);
  return rules;
}

try {
  const page = await browser.newPage({ viewport: { width: WIDTH, height: HEIGHT }, deviceScaleFactor: SCALE });
  await page.goto(`${base}/?film=1&hero=0`, { waitUntil: "load" });
  await page.waitForFunction(() => document.documentElement.dataset.stage === "ready", null, { timeout: 30_000 });
  await page.waitForFunction(() => window.BuildFilm.stage.getState().screens.laptop?.includes("ui09-needs-you"), null, { timeout: 30_000 });
  await page.waitForTimeout(1500);
  await page.addStyleTag({ content: "html, body { background: transparent !important; } .site-nav, .act, .overlays { visibility: hidden !important; }" });
  await page.evaluate(() => window.BuildFilm.sync());
  await page.waitForTimeout(300);
  const shot = await page.screenshot({ omitBackground: true });
  const crop = await cropToHardware(page, shot);
  if (crop.width < 100) throw new Error("The capture found no hardware on the stage.");
  await fs.writeFile(image, Buffer.from(crop.webp.split(",")[1], "base64"));
  const quads = await screenQuads(page);
  await fs.writeFile(anchors, anchorsModule(crop, quads));
  const rules = await writePlacement(crop);
  console.log(JSON.stringify({ image: image.pathname, size: [crop.width, crop.height], rules }));
} finally {
  await browser.close();
}
