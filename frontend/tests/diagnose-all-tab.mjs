// Diagnose why the "All" tab doesn't populate.
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
await page.goto(`${BASE}/dashboard/`, { waitUntil: 'domcontentloaded' });
await page.waitForSelector('.v2-channel-sidebar-item');
await page.click('.v2-channel-sidebar-item');
await page.waitForTimeout(1500);

// Dump state BEFORE clicking All.
console.log('--- BEFORE All click ---');
console.log(await page.evaluate(() => {
  const d = window.__v2debug;
  const id = d.stores.uiStore.getActiveChannel();
  return {
    activeTab: document.querySelector('.v2-files-tree-tab.active')?.getAttribute('data-tree-tab'),
    viewStateTab: d.channelRegistry.active?.viewState?.filesTreeTab,
    treeKeys: [...d.stores.filesStore.treeFor(id).keys()],
    treeBodyInnerHTML: document.querySelector('.v2-files-tree-body')?.innerHTML.slice(0, 400),
  };
}));

// Click All.
await page.click('[data-tree-tab="all"]');
await page.waitForTimeout(500);

console.log('\n--- AFTER All click ---');
console.log(await page.evaluate(() => {
  const d = window.__v2debug;
  const id = d.stores.uiStore.getActiveChannel();
  return {
    activeTab: document.querySelector('.v2-files-tree-tab.active')?.getAttribute('data-tree-tab'),
    viewStateTab: d.channelRegistry.active?.viewState?.filesTreeTab,
    treeKeys: [...d.stores.filesStore.treeFor(id).keys()],
    treeBodyInnerHTML: document.querySelector('.v2-files-tree-body')?.innerHTML.slice(0, 600),
  };
}));

await browser.close();
