// Verify that the tree-tabs row and the path bar sit at the same
// vertical top + bottom, so the horizontal divider across the
// viewer lines up.
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';

const browser = await chromium.launch({ headless: true });
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await ctx.newPage();

let passed = 0, failed = 0;
function check(label, ok, note) {
  if (ok) { passed++; console.log(`[PASS] ${label}${note ? ` — ${note}` : ''}`); }
  else    { failed++; console.log(`[FAIL] ${label}${note ? ` — ${note}` : ''}`); }
}

try {
  await page.goto(`${BASE}/auth/dummy/login`, { waitUntil: 'domcontentloaded' });
  await page.fill('input[name="email"]', EMAIL);
  const n = await page.$('input[name="name"]');
  if (n) await page.fill('input[name="name"]', 'Dev');
  await Promise.all([
    page.waitForURL(u => !u.pathname.startsWith('/auth/'), { timeout: 10000 }).catch(() => null),
    page.click('button[type="submit"]'),
  ]);
  await page.goto(`${BASE}/dashboard/`, { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });
  const chId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  await page.click(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
  await page.waitForTimeout(400);

  const rects = await page.evaluate(() => {
    const tabs = document.querySelector('.v2-files-tree-tabs');
    const path = document.getElementById('v2-path-bar');
    return {
      tabs: tabs?.getBoundingClientRect().toJSON(),
      path: path?.getBoundingClientRect().toJSON(),
    };
  });

  check('tabs row found',      !!rects.tabs,                  JSON.stringify(rects.tabs));
  check('path bar found',      !!rects.path,                  JSON.stringify(rects.path));
  check('tops align (±1px)',   Math.abs(rects.tabs.top - rects.path.top) <= 1,
    `tabs.top=${rects.tabs.top} path.top=${rects.path.top}`);
  check('bottoms align (±1px)', Math.abs(rects.tabs.bottom - rects.path.bottom) <= 1,
    `tabs.bottom=${rects.tabs.bottom} path.bottom=${rects.path.bottom}`);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
