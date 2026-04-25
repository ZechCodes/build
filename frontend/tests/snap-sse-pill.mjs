// Snap the reconnecting pill visible at the bottom of the viewport.
import { chromium } from 'playwright';
const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';

const browser = await chromium.launch({ headless: true });
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const p = await ctx.newPage();
await p.goto(`${BASE}/auth/dummy/login`, { waitUntil: 'domcontentloaded' });
await p.fill('input[name="email"]', EMAIL);
const n = await p.$('input[name="name"]'); if (n) await p.fill('input[name="name"]', 'Dev');
await Promise.all([
  p.waitForURL(u => !u.pathname.startsWith('/auth/'), { timeout: 10000 }).catch(() => null),
  p.click('button[type="submit"]'),
]);
await p.goto(`${BASE}/dashboard/`, { waitUntil: 'domcontentloaded' });
await p.waitForSelector('.sk-status-indicator', { timeout: 8000 });

await p.evaluate(() => {
  const el = document.querySelector('.sk-status-indicator');
  el.classList.remove('sk-status-indicator-hidden');
  el.querySelector('.sk-status-label').textContent = 'Reconnecting…';
  el.querySelector('.sk-status-dot').style.background = 'var(--sk-color-warning)';
});
await p.waitForTimeout(300);
// Clip to bottom-center of viewport.
await p.screenshot({ path: 'sse-pill.png', clip: { x: 420, y: 780, width: 600, height: 110 } });
console.log('wrote sse-pill.png');
await browser.close();
