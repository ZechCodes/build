// Capture the hero cutout the film shows until its first WebGL frame: the
// live stage at the hero pose, lid where the welcome starts, on a transparent
// background, cropped to the hardware. Writes the image and the placement
// numbers film.css reads, so the two cannot drift apart. Start
// scripts/preview-landing.py on a fresh `npm run build` first.
//
//   LANDING_URL    where the preview listens (default http://127.0.0.1:4173)
//   CHROMIUM_PATH  a system Chromium instead of Playwright's download
import fs from "node:fs/promises";
import { chromium } from "playwright";
import { POSES } from "../landing/src/film/acts.js";

const base = process.env.LANDING_URL || "http://127.0.0.1:4173";
const WIDTH = 1440;
const HEIGHT = 900;
const SCALE = 2;
const LID_ENTRANCE_START = 0.7;
const image = new URL("../skriftapp/buildapp/landing/assets/devices/hero-entrance.webp", import.meta.url);
const css = new URL("../landing/src/styles/film.css", import.meta.url);

const browser = await chromium.launch({
  headless: true,
  executablePath: process.env.CHROMIUM_PATH,
  args: ["--headless=new", "--use-gl=angle", "--use-angle=gl", "--ignore-gpu-blocklist"],
});
try {
  const page = await browser.newPage({ viewport: { width: WIDTH, height: HEIGHT }, deviceScaleFactor: SCALE });
  await page.goto(`${base}/?film=1`, { waitUntil: "load" });
  await page.waitForFunction(() => document.documentElement.dataset.stage === "ready", null, { timeout: 30_000 });
  await page.waitForTimeout(2500);
  await page.addStyleTag({ content: "html, body { background: transparent !important; } .site-nav, .act, .overlays, .hero-poster { visibility: hidden !important; }" });
  await page.evaluate((lid) => {
    window.BuildFilm.pose.laptop.lidOpen = lid;
    window.BuildFilm.sync();
  }, LID_ENTRANCE_START);
  await page.waitForTimeout(300);
  const shot = await page.screenshot({ omitBackground: true });
  const crop = await page.evaluate(async (dataUrl) => {
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
  if (crop.width < 100) throw new Error("The capture found no hardware on the stage.");
  await fs.writeFile(image, Buffer.from(crop.webp.split(",")[1], "base64"));

  // The stage sizes the laptop by the width and centres it at x% of the width
  // and y% of the height; everything else about the cutout scales with the
  // width too.
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
    if (!pattern.test(sheet)) throw new Error(`film.css has no ${name}`);
    sheet = sheet.replace(pattern, `$1${value};`);
  }
  await fs.writeFile(css, sheet);
  console.log(JSON.stringify({ image: image.pathname, size: [crop.width, crop.height], rules }));
} finally {
  await browser.close();
}
