// Capture overlay with messages loaded + composer visible.
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await context.newPage();

await page.goto(`${BASE}/auth/dummy/login`, { waitUntil: 'domcontentloaded' });
await page.fill('input[name="email"]', EMAIL);
const nameInput = await page.$('input[name="name"]');
if (nameInput) await page.fill('input[name="name"]', 'Dev');
await Promise.all([
  page.waitForURL(u => !u.pathname.startsWith('/auth/'), { timeout: 10000 }).catch(() => null),
  page.click('button[type="submit"]'),
]);
await page.goto(`${BASE}/dashboard-v2/`, { waitUntil: 'domcontentloaded' });
await page.waitForTimeout(3000);
await page.waitForSelector('.v2-channel-sidebar-item');
await page.click('.v2-channel-sidebar-item');
await page.waitForTimeout(1500);
await page.click('#v2-rail-chat-toggle');    // open overlay
await page.waitForTimeout(800);

// Assert composer + messages both visible.
const dims = await page.evaluate(() => {
  const overlay = document.getElementById('v2-chat-overlay');
  const body = document.getElementById('v2-chat-overlay-body');
  const composer = document.querySelector('.v2-chat-composer');
  const msgs = document.querySelector('.v2-chat-messages');
  const msgCount = document.querySelectorAll('#v2-chat-overlay-body .v2-msg').length;
  const ovRect = overlay.getBoundingClientRect();
  const cRect = composer?.getBoundingClientRect();
  const msgsRect = msgs?.getBoundingClientRect();
  return {
    overlay: { top: ovRect.top, bottom: ovRect.bottom, h: ovRect.height, w: ovRect.width },
    composer: cRect ? { top: cRect.top, bottom: cRect.bottom, h: cRect.height } : null,
    messages: msgsRect ? { top: msgsRect.top, bottom: msgsRect.bottom, h: msgsRect.height, scrollH: msgs.scrollHeight, clientH: msgs.clientHeight } : null,
    msgCount,
  };
});
console.log(JSON.stringify(dims, null, 2));

await page.screenshot({ path: 'overlay.png', fullPage: false });
console.log('screenshot: overlay.png');

await browser.close();
