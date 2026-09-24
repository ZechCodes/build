import { chromium } from "playwright";
const browser = await chromium.launch({ executablePath: "/usr/bin/chromium" });
for (const [file, width, height] of [["compose-390", 390, 760], ["compose-1440", 1440, 620]]) {
  const p = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: 2 });
  p.on("pageerror", (e) => console.log("PAGEERROR", e.message));
  await p.goto(`http://localhost:4188/app/static/__${file}.html`);
  await p.waitForFunction(() => window.__ready === true, { timeout: 8000 });
  await p.waitForTimeout(250);
  await p.screenshot({ path: `/tmp/rowshot/${file}.png` });
  await p.close();
  console.log(file);
}
await browser.close();
