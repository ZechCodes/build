// Screenshots the pages render-list.mjs wrote. See README.md beside it.
import { chromium } from "playwright";

const browser = await chromium.launch({ executablePath: "/usr/bin/chromium" });
const shots = [];
for (const prefix of ["before", "after"]) {
  for (const [width, theme] of [[390, "dark"], [1440, "dark"], [390, "light"], [1440, "light"]]) {
    const page = await browser.newPage({ viewport: { width, height: 900 }, deviceScaleFactor: 2 });
    await page.goto(`file:///tmp/rowshot/${prefix}-${width}-${theme}.html`);
    await page.waitForTimeout(150);
    const out = `/tmp/rowshot/${prefix}-${width}-${theme}.png`;
    await page.screenshot({ path: out, fullPage: true });
    shots.push(out);
    await page.close();
  }
}
await browser.close();
console.log(shots.join("\n"));
