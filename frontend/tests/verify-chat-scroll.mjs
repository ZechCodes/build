// Verify chat new-message scroll behavior:
//   short message  → scrolled to bottom (last msg fully visible)
//   long message   → top of msg at top of viewport
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await context.newPage();

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
  await page.waitForSelector('.v2-channel-sidebar-item');
  const chId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  await page.click(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
  await page.waitForTimeout(1500);

  // Ensure overlay is open.
  if (!(await page.$eval('#v2-chat-overlay', el => el.classList.contains('open')))) {
    await page.click('#v2-rail-chat-toggle'); await page.waitForTimeout(300);
  }

  // Case 0: initial render → overlay should already be scrolled to bottom
  // (the channel has 13+ cached messages).
  const initial = await page.evaluate(() => {
    const c = document.querySelector('.v2-chat-messages');
    if (!c) return null;
    return { top: c.scrollTop, max: c.scrollHeight - c.clientHeight, h: c.clientHeight };
  });
  check('initial render scrolls to bottom',
    initial && initial.max > 0 && Math.abs(initial.max - initial.top) < 10,
    JSON.stringify(initial));

  // Case 1: append a SHORT message → expect scroll to bottom.
  await page.evaluate((id) => {
    window.__v2debug.stores.messagesStore.append(id, {
      id: 's-' + Date.now(), channel_id: id, sender: 'Device',
      content: 'short msg', created_at: new Date().toISOString(),
    });
  }, chId);
  await page.waitForTimeout(200);
  const shortPos = await page.evaluate(() => {
    const c = document.querySelector('.v2-chat-messages');
    return { top: c.scrollTop, max: c.scrollHeight - c.clientHeight };
  });
  check('short msg → scrolled to bottom', Math.abs(shortPos.max - shortPos.top) < 4, JSON.stringify(shortPos));

  // Case 2: append a VERY LONG message (forces height > viewport)
  // and check that its top aligns with the container's scrollTop.
  const longContent = Array.from({ length: 400 }, (_, i) => `line ${i}`).join('\n');
  await page.evaluate(({ id, content }) => {
    window.__v2debug.stores.messagesStore.append(id, {
      id: 'l-' + Date.now(), channel_id: id, sender: 'Device',
      content, created_at: new Date().toISOString(),
    });
  }, { id: chId, content: longContent });
  await page.waitForTimeout(300);

  const longInfo = await page.evaluate(() => {
    const c = document.querySelector('.v2-chat-messages');
    const last = c.lastElementChild;
    const cRect = c.getBoundingClientRect();
    const lRect = last.getBoundingClientRect();
    return {
      scrollTop: c.scrollTop,
      offsetTop: last.offsetTop,
      msgH: last.offsetHeight,
      viewportH: c.clientHeight,
      topsAlign: Math.abs(cRect.top - lRect.top) < 4,
    };
  });
  console.log('  long info:', longInfo);
  check('long msg taller than viewport', longInfo.msgH > longInfo.viewportH);
  check('long msg top aligns with viewport top', longInfo.topsAlign, JSON.stringify(longInfo));

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
