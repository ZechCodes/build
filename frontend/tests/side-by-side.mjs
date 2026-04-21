// Capture v1 and v2 screenshots side-by-side in the same state and
// composite them for visual comparison.

import { chromium } from 'playwright';
import { readFileSync } from 'node:fs';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';
const VIEWPORT = { width: 1440, height: 900 };
const browser = await chromium.launch({ headless: true });

async function capture(path, file) {
  const ctx = await browser.newContext({ viewport: VIEWPORT });
  const page = await ctx.newPage();
  await page.goto(`${BASE}/auth/dummy/login`, { waitUntil: 'domcontentloaded' });
  await page.fill('input[name="email"]', EMAIL);
  const n = await page.$('input[name="name"]');
  if (n) await page.fill('input[name="name"]', 'Dev');
  await Promise.all([
    page.waitForURL(u => !u.pathname.startsWith('/auth/'), { timeout: 10000 }).catch(() => null),
    page.click('button[type="submit"]'),
  ]);

  await page.goto(`${BASE}${path}`, { waitUntil: 'domcontentloaded' });
  await page.waitForTimeout(3000);

  // Select first channel in each.
  if (path.includes('-v2')) {
    const row = await page.$('.v2-channel-sidebar-item');
    if (row) await row.click();
    await page.waitForTimeout(1500);
    // Ensure chat overlay is open.
    await page.click('#v2-rail-chat-toggle').catch(() => {});
    await page.waitForTimeout(500);
  } else {
    const row = await page.$('.channel-sidebar-item');
    if (row) await row.click();
    await page.waitForTimeout(1500);
    // Ensure overlay open in v1.
    await page.click('#console-chat-toggle').catch(() => {});
    await page.waitForTimeout(500);
  }
  await page.screenshot({ path: file, fullPage: false });
  await ctx.close();
}

await capture('/dashboard/', 'diff-v1.png');
await capture('/dashboard-v2/', 'diff-v2.png');

// Compose side-by-side image: render an HTML page with the two PNGs
// as background, then screenshot the composite.
const v1b64 = readFileSync('diff-v1.png').toString('base64');
const v2b64 = readFileSync('diff-v2.png').toString('base64');

const ctx = await browser.newContext({ viewport: { width: VIEWPORT.width * 2 + 40, height: VIEWPORT.height + 60 } });
const page = await ctx.newPage();
await page.setContent(`
  <html><head><style>
    body { margin: 0; background: #222; font-family: -apple-system, sans-serif; color: #ccc; }
    .row { display: flex; gap: 12px; padding: 14px; }
    .col { width: ${VIEWPORT.width}px; text-align: center; }
    .label { padding: 6px; font-size: 13px; }
    img { width: 100%; display: block; border: 1px solid #444; }
  </style></head><body>
    <div class="row">
      <div class="col">
        <div class="label">v1 /dashboard</div>
        <img src="data:image/png;base64,${v1b64}">
      </div>
      <div class="col">
        <div class="label">v2 /dashboard-v2</div>
        <img src="data:image/png;base64,${v2b64}">
      </div>
    </div>
  </body></html>
`);
await page.waitForLoadState('load');
await page.screenshot({ path: 'side-by-side.png', fullPage: true });
console.log('side-by-side.png');

// Diff overlay via mix-blend-mode: difference.
const ctx2 = await browser.newContext({ viewport: VIEWPORT });
const page2 = await ctx2.newPage();
await page2.setContent(`
  <html><head><style>
    body { margin: 0; background: #000; }
    .stack { position: relative; width: ${VIEWPORT.width}px; height: ${VIEWPORT.height}px; }
    .stack img { position: absolute; inset: 0; width: 100%; height: 100%; }
    .diff { mix-blend-mode: difference; }
  </style></head><body>
    <div class="stack">
      <img src="data:image/png;base64,${v1b64}">
      <img class="diff" src="data:image/png;base64,${v2b64}">
    </div>
  </body></html>
`);
await page2.waitForLoadState('load');
await page2.screenshot({ path: 'diff-overlay.png', fullPage: false });
console.log('diff-overlay.png');

await browser.close();
