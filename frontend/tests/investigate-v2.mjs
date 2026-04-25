// Ad-hoc investigation: open /dashboard/ as dev@local, capture what's
// actually happening — console errors, stores, DOM, network.

import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';

const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await context.newPage();

const consoleLogs = [];
const pageErrors = [];
const networkBad = [];
page.on('console', (msg) => {
  consoleLogs.push(`[${msg.type()}] ${msg.text()}`);
});
page.on('pageerror', (err) => {
  pageErrors.push(`${err.name}: ${err.message}\n${err.stack || ''}`);
});
page.on('response', (resp) => {
  if (resp.status() >= 400 && !resp.url().includes('/auth/')) networkBad.push(`${resp.status()} ${resp.url()}`);
});

try {
  // Dummy login
  await page.goto(`${BASE}/auth/dummy/login`, { waitUntil: 'domcontentloaded' });
  await page.fill('input[name="email"]', EMAIL);
  const nameInput = await page.$('input[name="name"]');
  if (nameInput) await page.fill('input[name="name"]', 'Dev');
  await Promise.all([
    page.waitForURL((u) => !u.pathname.startsWith('/auth/'), { timeout: 10000 }).catch(() => null),
    page.click('button[type="submit"]'),
  ]);
  console.log('After login URL:', page.url());

  // Navigate to v2
  await page.goto(`${BASE}/dashboard/`, { waitUntil: 'domcontentloaded' });
  await page.waitForTimeout(4000);   // give E2EE time
  console.log('v2 URL:', page.url());

  // Core DOM presence
  const dom = await page.evaluate(() => ({
    hasApp: !!document.querySelector('.v2-app'),
    hasSidebar: !!document.getElementById('v2-sidebar'),
    hasChannelPanelList: !!document.getElementById('v2-channel-panel-list'),
    channelListInnerHTML: document.getElementById('v2-channel-panel-list')?.innerHTML?.slice(0, 800) || null,
    hasChatTabPanel: !!document.getElementById('v2-tab-chat'),
    chatInnerHTML: document.getElementById('v2-tab-chat')?.innerHTML?.slice(0, 400) || null,
    hasFilesTabPanel: !!document.getElementById('v2-tab-files'),
    activeTabBody: document.body.dataset.tab,
    bundleJS: [...document.scripts].map(s => s.src).filter(Boolean),
    bundleCSS: [...document.querySelectorAll('link[rel=stylesheet]')].map(l => l.href).filter(Boolean),
  }));
  console.log('\n--- DOM ---');
  console.log(JSON.stringify(dom, null, 2));

  // Store state
  const state = await page.evaluate(() => {
    const d = window.__v2debug;
    if (!d) return { error: '__v2debug not found' };
    return {
      debugKeys: Object.keys(d),
      devices: d.stores.devicesStore.list(),
      channels: d.stores.channelsStore.list().map(c => ({ id: c.id, name: c.name, deviceId: d.stores.channelsStore.deviceFor(c.id) })),
      activeChannel: d.stores.uiStore.getActiveChannel(),
      tab: d.stores.uiStore.getTab(),
      e2eeStatus: d.e2eePool.status(),
      e2eeInstances: [...d.e2eePool._instances.keys?.() || []],
    };
  });
  console.log('\n--- v2 stores ---');
  console.log(JSON.stringify(state, null, 2));

  // Network /api/devices/ result
  const devicesApi = await page.evaluate(async () => {
    const r = await fetch('/api/devices/');
    return { status: r.status, body: await r.text().then(s => s.slice(0, 300)) };
  });
  console.log('\n--- /api/devices/ ---');
  console.log(JSON.stringify(devicesApi));

  console.log('\n--- Console (last 40) ---');
  for (const line of consoleLogs.slice(-40)) console.log(line);
  console.log('\n--- Page errors ---');
  for (const e of pageErrors) console.log(e, '\n---');
  console.log('\n--- Bad responses (>=400) ---');
  for (const n of networkBad) console.log(n);

} finally {
  await browser.close();
}
