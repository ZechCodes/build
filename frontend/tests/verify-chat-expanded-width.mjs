// Verify the expanded-mode chat overlay caps its width on wide
// desktops (for plan readability) but still fills the available
// space on narrower ones.
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';

async function measure({ width, height }) {
  const browser = await chromium.launch({ headless: true });
  const ctx = await browser.newContext({ viewport: { width, height } });
  const page = await ctx.newPage();
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
  await page.waitForTimeout(300);

  // Make sure the overlay is open + flip it to expanded.
  if (!(await page.$eval('#v2-chat-overlay', el => el.classList.contains('open')))) {
    await page.click('#v2-rail-chat-toggle');
    await page.waitForTimeout(200);
  }
  await page.evaluate(() => {
    document.getElementById('v2-chat-overlay').setAttribute('data-mode', 'expanded');
  });
  await page.waitForTimeout(100);

  const rect = await page.$eval('#v2-chat-overlay', el => {
    const r = el.getBoundingClientRect();
    return { width: Math.round(r.width), height: Math.round(r.height), left: Math.round(r.left), right: Math.round(r.right) };
  });
  await browser.close();
  return rect;
}

let passed = 0, failed = 0;
function check(label, ok, note) {
  if (ok) { passed++; console.log(`[PASS] ${label}${note ? ` — ${note}` : ''}`); }
  else    { failed++; console.log(`[FAIL] ${label}${note ? ` — ${note}` : ''}`); }
}

try {
  // On a 2000×1000 monitor, the overlay should cap at 900px wide.
  const wide = await measure({ width: 2000, height: 1000 });
  check('2000w: expanded overlay capped at 900px',
    wide.width === 900, JSON.stringify(wide));

  // On a 1100×900 monitor (sidebar 260 + gaps ~32 = available ~808),
  // the overlay stays at 808px (min wins).
  const narrow = await measure({ width: 1100, height: 900 });
  check('1100w: expanded overlay fills available space (≈808px)',
    narrow.width < 900 && narrow.width > 700, JSON.stringify(narrow));

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} catch (err) {
  console.error('test crashed:', err);
  process.exitCode = 1;
}
