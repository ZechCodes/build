// Verify the chat shows a permanent divider when the agent resets
// or compacts its session.
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
  await page.goto(`${BASE}/dashboard-v2/`, { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });
  const chId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  await page.click(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
  await page.waitForTimeout(400);
  if (!(await page.$eval('#v2-chat-overlay', el => el.classList.contains('open')))) {
    await page.click('#v2-rail-chat-toggle');
    await page.waitForTimeout(250);
  }

  // Fire session.reset + session.compacting; each should append a
  // divider to the chat history.
  await page.evaluate((chId) => {
    window.__v2debug.bus.emit('session.reset',      { channelId: chId });
    window.__v2debug.bus.emit('session.compacting', { channelId: chId });
  }, chId);
  await page.waitForTimeout(200);

  const dividers = await page.evaluate(() => {
    const els = [...document.querySelectorAll('.v2-session-divider')];
    return els.map(el => ({
      label: el.querySelector('.v2-session-divider-label')?.textContent?.trim(),
      isReset:   el.classList.contains('v2-session-divider-reset'),
      isCompact: el.classList.contains('v2-session-divider-compact'),
    }));
  });
  check('at least 2 dividers rendered',       dividers.length >= 2, JSON.stringify(dividers));
  const reset = dividers.find(d => d.isReset);
  const compact = dividers.find(d => d.isCompact);
  check('reset divider rendered with label',     !!reset && /new session started/i.test(reset.label || ''),
    JSON.stringify(reset));
  check('compact divider rendered with label',   !!compact && /session compacted/i.test(compact.label || ''),
    JSON.stringify(compact));

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
