// Verify the sidebar account footer:
//   - renders user name + first-initial avatar,
//   - account link points at /admin/,
//   - theme-update button appears when /api/theme/version reports a
//     version newer than this page's in-memory baseline,
//   - persists across reconnects,
//   - clicking "Update" triggers a reload and clears the badge.
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

// Drive /api/theme/version from a variable we can flip at will.
let SERVED_VERSION = '1.0.0';
await ctx.route('**/api/theme/version', (route) =>
  route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({ version: SERVED_VERSION }),
  }));

async function forceRecheck(page) {
  await page.evaluate(() => window.__v2debug.bus.emit('sse.connected', {}));
  await page.waitForTimeout(250);
}

async function badgePresent(page) {
  return page.evaluate(() => !!document.querySelector('.v2-theme-update'));
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
  await page.waitForSelector('#v2-account-slot .v2-account', { timeout: 8000 });
  // Let the mount-time _checkThemeVersion seed the page-lifetime baseline.
  await page.waitForTimeout(300);

  // 1. Account renders with user name + initial.
  const account = await page.evaluate(() => {
    const el = document.querySelector('#v2-account-slot .v2-account');
    return el ? {
      href: el.getAttribute('href'),
      name: el.querySelector('.v2-account-name')?.textContent?.trim(),
      initial: el.querySelector('.v2-account-avatar')?.textContent?.trim(),
      title: el.getAttribute('title'),
    } : null;
  });
  check('account link renders',      !!account, JSON.stringify(account));
  check('account href is /admin/',   account?.href === '/admin/');
  check('account name is non-empty', !!account?.name && account.name.length > 0);
  check('avatar is single uppercase initial',
    /^[A-Z?]$/.test(account?.initial || ''), account?.initial);
  check('title attr matches name',   account?.title === account?.name);

  // 2. No badge initially — mount's own check just seeded the baseline.
  check('no update button at mount', !(await badgePresent(page)));

  // 3. Simulate a stale tab: server starts reporting a newer version,
  //    then sse.connected asks the account view to compare against its
  //    original in-memory baseline.
  SERVED_VERSION = '1.1.0';
  await forceRecheck(page);
  check('update button appears when server > page baseline', await badgePresent(page));

  // 4. Reconnecting again must keep the badge.
  await forceRecheck(page);
  check('update button persists across reconnects', await badgePresent(page));

  // 5. Clicking the update button triggers a full reload.
  const loadPromise = page.waitForEvent('load', { timeout: 3000 })
    .then(() => true).catch(() => false);
  await page.click('.v2-theme-update');
  const reloaded = await loadPromise;
  check('clicking update triggers a page reload', reloaded === true);
  await page.waitForSelector('#v2-account-slot .v2-account', { timeout: 8000 });
  await page.waitForTimeout(300);
  check('no update button after reload baseline refreshes', !(await badgePresent(page)));

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
