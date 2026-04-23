// Verify the channel menu in the overlay header:
//   - "…" button (#v2-co-menu) is rendered
//   - clicking it opens a body-level `.v2-channel-menu` popover
//   - Restart / Stop actions fire their intents
//   - Rename replaces the title with an input; Enter commits via
//     `intent.update_channel` with a `name` patch, Esc cancels
//   - Outside click closes the menu
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';

const browser = await chromium.launch({ headless: true });
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await ctx.newPage();

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
  await page.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });
  const chId = await page.evaluate(() => {
    const el = document.querySelector('.v2-sidebar .v2-channel-sidebar-item');
    const id = el?.getAttribute('data-channel-id');
    if (id) window.__v2debug.stores.uiStore.setActiveChannel(id);
    return id;
  });
  await page.waitForTimeout(300);
  if (!(await page.$eval('#v2-chat-overlay', el => el.classList.contains('open')))) {
    await page.click('#v2-rail-chat-toggle');
    await page.waitForTimeout(250);
  }

  // 1. Menu button exists in the overlay header.
  const hasBtn = await page.$('#v2-co-menu') != null;
  check('overlay header has a menu button', hasBtn);

  // 2. Clicking it opens a menu with Restart / Stop / Rename.
  await page.click('#v2-co-menu');
  await page.waitForTimeout(60);
  const menuState = await page.evaluate(() => {
    const menu = document.querySelector('.v2-channel-menu');
    if (!menu) return null;
    return {
      parentTag: menu.parentElement?.tagName,
      actions: [...menu.querySelectorAll('[data-ch-action]')].map(b => b.getAttribute('data-ch-action')),
    };
  });
  check('menu opens on click',                   menuState != null);
  check('menu is a body-level popover',          menuState?.parentTag === 'BODY');
  check('menu has restart/stop/rename actions',
    JSON.stringify(menuState?.actions) === JSON.stringify(['restart', 'stop', 'rename']),
    JSON.stringify(menuState?.actions));

  // 3. Restart fires intent.restart_agent and closes the menu.
  await page.evaluate(() => {
    window.__lastRestart = null;
    window.__v2debug.bus.on('intent.restart_agent', (p) => { window.__lastRestart = p; });
  });
  await page.click('[data-ch-action="restart"]');
  await page.waitForTimeout(80);
  const restarted = await page.evaluate(() => window.__lastRestart);
  check('Restart fires intent.restart_agent with channelId',
    restarted?.channelId === chId, JSON.stringify(restarted));
  const menuGone = await page.$('.v2-channel-menu') == null;
  check('menu closes after action click', menuGone);

  // 4. Stop fires intent.stop_agent.
  await page.click('#v2-co-menu');
  await page.waitForTimeout(40);
  await page.evaluate(() => {
    window.__lastStop = null;
    window.__v2debug.bus.on('intent.stop_agent', (p) => { window.__lastStop = p; });
  });
  await page.click('[data-ch-action="stop"]');
  await page.waitForTimeout(80);
  const stopped = await page.evaluate(() => window.__lastStop);
  check('Stop fires intent.stop_agent', stopped?.channelId === chId, JSON.stringify(stopped));

  // 5. Rename → input appears, Enter fires intent.update_channel.
  await page.click('#v2-co-menu');
  await page.waitForTimeout(40);
  await page.evaluate(() => {
    window.__lastUpdate = null;
    window.__v2debug.bus.on('intent.update_channel', (p) => { window.__lastUpdate = p; });
  });
  await page.click('[data-ch-action="rename"]');
  await page.waitForTimeout(60);
  const hasInput = await page.$('.v2-co-rename-input') != null;
  check('rename reveals an input', hasInput);
  await page.locator('.v2-co-rename-input').fill('Renamed');
  await page.locator('.v2-co-rename-input').press('Enter');
  await page.waitForTimeout(120);
  const updated = await page.evaluate(() => window.__lastUpdate);
  check('rename commit fires intent.update_channel with name patch',
    updated?.channelId === chId && updated?.patch?.name === 'Renamed',
    JSON.stringify(updated));

  // 6. Esc on rename cancels without firing update_channel.
  await page.click('#v2-co-menu');
  await page.waitForTimeout(40);
  await page.click('[data-ch-action="rename"]');
  await page.waitForTimeout(40);
  await page.evaluate(() => { window.__lastUpdate = null; });
  await page.locator('.v2-co-rename-input').fill('Nope');
  await page.locator('.v2-co-rename-input').press('Escape');
  await page.waitForTimeout(80);
  const cancel = await page.evaluate(() => window.__lastUpdate);
  check('Esc on rename cancels — no update intent', cancel === null);

  // 7. Outside click closes the menu.
  await page.click('#v2-co-menu');
  await page.waitForTimeout(40);
  await page.mouse.click(5, 5);
  await page.waitForTimeout(80);
  const gone = await page.$('.v2-channel-menu') == null;
  check('outside click closes the menu', gone);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
