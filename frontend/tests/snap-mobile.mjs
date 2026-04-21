// Snapshot v2 at iPhone viewport at three states:
//   1. initial (files view, sidebar hidden)
//   2. drawer open (after hamburger tap)
//   3. chat overlay fullscreen
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';
const browser = await chromium.launch({ headless: true });
const ctx = await browser.newContext({
  viewport: { width: 375, height: 812 },
  isMobile: true,
  hasTouch: true,
});
const p = await ctx.newPage();

await p.goto(`${BASE}/auth/dummy/login`, { waitUntil: 'domcontentloaded' });
await p.fill('input[name="email"]', EMAIL);
const n = await p.$('input[name="name"]'); if (n) await p.fill('input[name="name"]', 'Dev');
await Promise.all([
  p.waitForURL(u => !u.pathname.startsWith('/auth/'), { timeout: 10000 }).catch(() => null),
  p.click('button[type="submit"]'),
]);
await p.goto(`${BASE}/dashboard-v2/`, { waitUntil: 'domcontentloaded' });
await p.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });
const chId = await p.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));

// Activate a channel so the viewer has content.
await p.tap('#v2-path-bar-menu');
await p.waitForTimeout(200);
await p.tap(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
await p.waitForTimeout(400);

await p.screenshot({ path: 'mobile-initial.png' });
console.log('wrote mobile-initial.png');

// Drawer open
await p.tap('#v2-path-bar-menu');
await p.waitForTimeout(300);
await p.screenshot({ path: 'mobile-drawer-open.png' });
console.log('wrote mobile-drawer-open.png');
// Close via backdrop.
await p.touchscreen.tap(360, 400);
await p.waitForTimeout(300);

// Chat overlay fullscreen
await p.tap('#v2-rail-chat-toggle');
await p.waitForTimeout(400);
await p.screenshot({ path: 'mobile-chat.png' });
console.log('wrote mobile-chat.png');

await browser.close();
