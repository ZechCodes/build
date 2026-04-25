// Verify that on first load with a channel in the URL hash, history
// appears without requiring the user to click elsewhere and back.
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await context.newPage();

await page.goto(`${BASE}/auth/dummy/login`, { waitUntil: 'domcontentloaded' });
await page.fill('input[name="email"]', EMAIL);
const n = await page.$('input[name="name"]');
if (n) await page.fill('input[name="name"]', 'Dev');
await Promise.all([
  page.waitForURL(u => !u.pathname.startsWith('/auth/'), { timeout: 10000 }).catch(() => null),
  page.click('button[type="submit"]'),
]);

// Find the channel id first (by visiting v2 once).
await page.goto(`${BASE}/dashboard/`, { waitUntil: 'domcontentloaded' });
await page.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });
const chId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));

// Now fresh-load directly to that channel's hash.
await page.goto(`${BASE}/dashboard/#files/${chId}`, { waitUntil: 'domcontentloaded' });

// Poll for messages to appear, up to 10s.
let messagesCount = 0, treeCount = 0;
for (let i = 0; i < 50; i++) {
  const state = await page.evaluate(() => {
    const d = window.__v2debug;
    if (!d) return { msgs: 0, tree: 0 };
    const id = d.stores.uiStore.getActiveChannel();
    return {
      msgs: id ? d.stores.messagesStore.forChannel(id).length : 0,
      tree: id ? ((d.stores.filesStore.changesFor(id) || []).reduce((n, r) => n + (r.entries?.length || 0), 0)) : 0,
      active: id,
    };
  });
  messagesCount = state.msgs;
  treeCount = state.tree;
  if (state.msgs > 0) break;
  await page.waitForTimeout(200);
}

console.log(`channel: ${chId}`);
console.log(`messages after direct-hash load: ${messagesCount}`);
console.log(`modified entries: ${treeCount}`);
console.log(messagesCount > 0 ? 'PASS: history loaded without re-click' : 'FAIL: history did not load');

await browser.close();
process.exit(messagesCount > 0 ? 0 : 1);
