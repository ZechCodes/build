// Screenshot v1 /dashboard so we have a current source-of-truth reference.
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
await page.goto(`${BASE}/dashboard/`, { waitUntil: 'domcontentloaded' });
await page.waitForTimeout(3000);

// Select first channel to see it populated.
const firstCh = await page.$('.channel-sidebar-item');
if (firstCh) { await firstCh.click(); await page.waitForTimeout(1500); }
// Open chat overlay.
const chatBtn = await page.$('#console-chat-toggle');
if (chatBtn) { await chatBtn.click(); await page.waitForTimeout(800); }

await page.screenshot({ path: 'v1-current.png', fullPage: false });
console.log('saved v1-current.png');

// Dump the chat composer specifically (#chat-input-area) and the
// overlay toolbar.
const bits = await page.evaluate(() => {
  const inputArea = document.querySelector('.chat-input-area');
  const toolbar = document.querySelector('.chat-overlay-toolbar');
  return {
    inputArea: inputArea?.outerHTML || 'missing',
    toolbar: toolbar?.outerHTML || 'missing',
  };
});
console.log('\n--- CHAT INPUT AREA ---\n' + bits.inputArea);
console.log('\n--- OVERLAY TOOLBAR ---\n' + bits.toolbar);

await browser.close();
