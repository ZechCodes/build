// Check v2's files store + DOM state to diagnose the empty tree panel.
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await context.newPage();

const errors = [];
page.on('pageerror', e => errors.push(`PAGE: ${e.message}`));
page.on('console', m => { if (m.type() === 'error') errors.push(`CONS: ${m.text()}`); });

await page.goto(`${BASE}/auth/dummy/login`, { waitUntil: 'domcontentloaded' });
await page.fill('input[name="email"]', EMAIL);
const n = await page.$('input[name="name"]');
if (n) await page.fill('input[name="name"]', 'Dev');
await Promise.all([
  page.waitForURL(u => !u.pathname.startsWith('/auth/'), { timeout: 10000 }).catch(() => null),
  page.click('button[type="submit"]'),
]);
await page.goto(`${BASE}/dashboard/`, { waitUntil: 'domcontentloaded' });
await page.waitForTimeout(3000);

// Select first channel.
await page.waitForSelector('.v2-channel-sidebar-item');
await page.click('.v2-channel-sidebar-item');
await page.waitForTimeout(2500);

const state = await page.evaluate(() => {
  const d = window.__v2debug;
  const active = d.stores.uiStore.getActiveChannel();
  const tree = d.stores.filesStore.treeFor(active);
  const changes = d.stores.filesStore.changesFor(active);
  return {
    activeChannel: active,
    treeKeys: [...tree.keys()],
    treeRoot: tree.get(''),
    changesLen: changes.length,
    changesSample: changes.slice(0, 3),
    treePanelHTML: document.getElementById('v2-files-tree-panel')?.innerHTML?.slice(0, 1500) || null,
  };
});

console.log('--- v2 files state ---');
console.log(JSON.stringify(state, null, 2));

console.log('\n--- errors ---');
for (const e of errors) console.log(e);

await browser.close();
