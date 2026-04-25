import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8092';
const EMAIL = 'smoke@test.local';

const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await context.newPage();

const consoleErrors = [];
const pageErrors = [];
const networkErrors = [];
page.on('console', (msg) => {
  if (msg.type() !== 'error') return;
  const text = msg.text();
  if (text.startsWith('Failed to load resource:')) return;
  consoleErrors.push(text);
});
page.on('pageerror', (err) => {
  pageErrors.push(`${err.name}: ${err.message}`);
});
page.on('response', (resp) => {
  if (resp.status() >= 500) networkErrors.push(`${resp.status()} ${resp.url()}`);
});

let passed = 0;
let failed = 0;
function check(label, ok, note) {
  if (ok) {
    passed++;
    console.log(`[PASS] ${label}${note ? ` - ${note}` : ''}`);
  } else {
    failed++;
    console.log(`[FAIL] ${label}${note ? ` - ${note}` : ''}`);
  }
}

try {
  await page.goto(`${BASE}/auth/dummy/login`, { waitUntil: 'domcontentloaded' });
  await page.fill('input[name="email"]', EMAIL);
  const nameInput = await page.$('input[name="name"]');
  if (nameInput) await page.fill('input[name="name"]', 'Smoke User');
  await Promise.all([
    page.waitForURL((u) => !u.pathname.startsWith('/auth/'), { timeout: 10000 }).catch(() => null),
    page.click('button[type="submit"]'),
  ]);
  check('login redirects off auth', !page.url().includes('/auth/'), page.url());

  await page.goto(`${BASE}/dashboard/`, { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.v2-app', { timeout: 10000 });

  const assets = await page.evaluate(() => ({
    dataDashboard: document.documentElement.dataset.dashboard,
    links: [...document.querySelectorAll('link[rel="stylesheet"]')].map(l => l.href),
    scripts: [...document.querySelectorAll('script[src]')].map(s => s.src),
    inlineStyles: document.querySelectorAll('style').length,
  }));
  check('dashboard template marked as v2', assets.dataDashboard === 'v2', assets.dataDashboard);
  check('canonical dashboard.css link present',
    assets.links.some(h => h.includes('/static/build/dist/css/dashboard.css')));
  check('canonical dashboard.js script present',
    assets.scripts.some(s => s.includes('/static/build/dist/js/dashboard.js')));
  const oldAssetName = 'dashboard' + '-v2';
  check('old v2 assets absent',
    !assets.links.concat(assets.scripts).some(h => h.includes(oldAssetName)));
  check('no inline <style> left', assets.inlineStyles === 0);

  for (const sel of [
    '#app .v2-app',
    '#v2-sidebar',
    '#v2-channel-panel-list',
    '#v2-viewer',
    '#v2-files-tree-panel',
    '#v2-files-content-panel',
    '#v2-rail',
    '#v2-chat-overlay',
    '#v2-complications',
    '#v2-dropdown-layer',
  ]) {
    check(`element ${sel} exists`, (await page.$(sel)) !== null);
  }

  const debug = await page.evaluate(() => ({
    sodium: typeof window.sodium,
    v2debug: typeof window.__v2debug,
    bus: typeof window.__v2debug?.bus?.emit,
    router: typeof window.__v2debug?.router?.navigate,
    channelRegistry: typeof window.__v2debug?.channelRegistry?.activate,
    uiStore: typeof window.__v2debug?.stores?.uiStore?.getActiveChannel,
    e2eePool: typeof window.__v2debug?.e2eePool?.connect,
  }));
  check('libsodium loaded', debug.sodium === 'object', debug.sodium);
  for (const [name, type] of Object.entries(debug)) {
    if (name === 'sodium') continue;
    check(`window.__v2debug.${name} exposed`, type === 'function' || type === 'object', type);
  }

  const bgColor = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
  check('body has computed background from bundled CSS',
    bgColor && bgColor !== 'rgba(0, 0, 0, 0)', bgColor);

  await page.evaluate(() => {
    window.__v2debug.bus.emit('device.bulk', {
      devices: [{ id: 'dev-smoke', name: 'Smoke Device', status: 'online', has_transport_key: true }],
    });
    window.__v2debug.bus.emit('channel.list', {
      deviceId: 'dev-smoke',
      channels: [{ id: 'ch-smoke', name: 'smoke', created_at: Date.now() }],
    });
  });
  await page.waitForSelector('.v2-channel-sidebar-item[data-channel-id="ch-smoke"]', { timeout: 5000 });
  check('synthetic channel renders',
    (await page.$('.v2-channel-sidebar-item[data-channel-id="ch-smoke"]')) !== null);

  await page.click('.v2-channel-sidebar-item[data-channel-id="ch-smoke"]');
  await page.waitForFunction(() => window.__v2debug.stores.uiStore.getActiveChannel() === 'ch-smoke');
  check('channel click updates active channel',
    await page.evaluate(() => window.__v2debug.stores.uiStore.getActiveChannel() === 'ch-smoke'));

  check('no page errors', pageErrors.length === 0, pageErrors.join('; '));
  check('no 500 responses', networkErrors.length === 0, networkErrors.join('; '));
  check('no console errors', consoleErrors.length === 0, consoleErrors.join('; '));
} catch (err) {
  failed++;
  console.error('[FAIL] smoke threw', err);
} finally {
  await browser.close();
}

console.log(`\nSmoke result: ${passed} passed, ${failed} failed`);
if (failed > 0) process.exit(1);
