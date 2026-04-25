// Verify that mobile tap targets — especially the rail chat toggle —
// carry `touch-action: manipulation` so browsers skip the 300ms
// double-tap-to-zoom wait after every tap. Before this, opening
// the chat on mobile felt laggy and users double-tapped, closing
// the overlay they just opened.
import { chromium, devices } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';

const browser = await chromium.launch({ headless: true });
const ctx = await browser.newContext({ ...devices['iPhone 14'] });
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
  await page.waitForSelector('#v2-rail-chat-toggle', { timeout: 8000 });

  // 1. Each of the common tap targets has touch-action: manipulation.
  const targets = [
    '#v2-rail-chat-toggle',
    '#v2-rail-terminal-toggle',
  ];
  for (const sel of targets) {
    const ta = await page.evaluate((s) => {
      const el = document.querySelector(s);
      return el ? getComputedStyle(el).touchAction : null;
    }, sel);
    check(`${sel} → touch-action: manipulation`, ta === 'manipulation', ta);
  }

  // 2. Channel row tap-target (if any exist on this device) also
  //    carries the rule.
  const chanTa = await page.evaluate(() => {
    const el = document.querySelector('.v2-channel-sidebar-item');
    return el ? getComputedStyle(el).touchAction : 'no-items';
  });
  if (chanTa !== 'no-items') {
    check('.v2-channel-sidebar-item → touch-action: manipulation', chanTa === 'manipulation', chanTa);
  }

  // 3. Tap the chat toggle; the overlay must flip to .open promptly.
  //    Playwright doesn't emulate the 300ms zoom wait, but this still
  //    checks that the synchronous toggle → apply path has no timers.
  const t0 = Date.now();
  await page.tap('#v2-rail-chat-toggle');
  await page.waitForFunction(
    () => document.getElementById('v2-chat-overlay').classList.contains('open'),
    { timeout: 1000 });
  const elapsed = Date.now() - t0;
  check(`overlay opens on first tap (< 500ms)`, elapsed < 500, `${elapsed}ms`);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
