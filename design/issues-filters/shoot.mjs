import { createRequire } from "node:module";
// Playwright comes from web/ (run `npm install` there first).
const require = createRequire(new URL("../../web/package.json", import.meta.url));
const { chromium } = require("playwright");
const browser = await chromium.launch({ executablePath: "/usr/bin/chromium" });
const shot = async (file, width, height, openMenu, search) => {
  const p = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: 2 });
  p.on("pageerror", (e) => console.log("PAGEERROR", e.message));
  await p.goto(`http://localhost:4188/app/static/__${file}.html`);
  await p.waitForFunction(() => window.__ready === true, { timeout: 8000 });
  await p.click(`[data-filter-menu="${openMenu}"] .fmenu-press`);
  if (search) await p.fill(`[data-filter-menu="${openMenu}"] .fmenu-search`, search);
  await p.waitForTimeout(200);
  await p.screenshot({ path: `/tmp/rowshot/${file}.png` });
  await p.close();
  console.log(file);
};
await shot("menu-390", 390, 720, "label", "");
await shot("menu-1440", 1440, 700, "assignee", "");
await shot("menu-1440-light", 1440, 700, "label", "tr");
await browser.close();
