// Verify the chat overlay closes on outside-click when unpinned,
// and stays open when pinned.
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

async function isOverlayOpen(page) {
  return page.$eval('#v2-chat-overlay', el => el.classList.contains('open'));
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
  await page.goto(`${BASE}/dashboard-v2/`, { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });
  const chId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  await page.click(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
  await page.waitForTimeout(300);
  if (!(await isOverlayOpen(page))) {
    await page.click('#v2-rail-chat-toggle');
    await page.waitForTimeout(200);
  }

  // ---- 1. Pinned → outside click keeps it open.
  await page.evaluate(() => window.__v2debug.stores.uiStore.setOverlayPinned(true));
  await page.waitForTimeout(50);
  await page.mouse.click(5, 5);
  await page.waitForTimeout(100);
  check('pinned overlay ignores outside click', await isOverlayOpen(page));

  // ---- 2. Unpinned → outside click closes.
  await page.evaluate(() => window.__v2debug.stores.uiStore.setOverlayPinned(false));
  await page.waitForTimeout(50);
  await page.mouse.click(5, 5);
  await page.waitForTimeout(100);
  check('unpinned overlay closes on outside click', !(await isOverlayOpen(page)));

  // ---- 3. Unpinned + click INSIDE overlay → stays open.
  await page.click('#v2-rail-chat-toggle');
  await page.waitForTimeout(150);
  // The rail click above may have flipped open via the toggle, which
  // respects pinned. Verify open, then click inside the overlay.
  await page.evaluate(() => window.__v2debug.stores.uiStore.setOverlayPinned(false));
  await page.waitForTimeout(50);
  const rect = await page.$eval('#v2-chat-overlay', el => {
    const r = el.getBoundingClientRect();
    return { x: r.left + 20, y: r.top + 20 };
  });
  await page.mouse.click(rect.x, rect.y);
  await page.waitForTimeout(100);
  check('unpinned + click inside overlay stays open', await isOverlayOpen(page));

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
