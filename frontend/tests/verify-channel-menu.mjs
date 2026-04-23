// Verify the per-channel actions menu:
//   - every channel row in the sidebar has a "…" trigger
//     ([data-channel-edit]) that opens a body-level .v2-channel-menu
//   - the menu offers Restart / Stop / Rename
//   - Restart / Stop fire their intents with that channel's id
//   - Rename replaces the overlay title with an inline input;
//     Enter commits via intent.update_channel, Esc cancels
//   - Clicking the "…" does NOT activate the channel (menu only)
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

  // Grab a real sidebar channel row — .v2-channel-sidebar-item is
  // reused by .v2-attention-row (same class, different parent), and
  // attention rows don't carry the edit trigger. Filter by the
  // presence of [data-channel-edit].
  const trigger = page.locator('.v2-sidebar [data-channel-edit]').first();
  const chId = await trigger.getAttribute('data-channel-edit');
  check('found a channel row with an edit trigger', !!chId, chId);
  check('channel row has a [data-channel-edit] trigger', await trigger.count() >= 1);

  // 2. Clicking it opens a body-level menu. Force the click so the
  //    invisible-until-hover opacity doesn't block it.
  // Spy: did the channel become active? It must NOT — the ... click
  // should be menu-only, not channel activation.
  await page.evaluate(() => {
    window.__activeAtStart = window.__v2debug.stores.uiStore.getActiveChannel();
  });
  await trigger.click({ force: true });
  await page.waitForTimeout(80);
  const menuState = await page.evaluate(() => {
    const menu = document.querySelector('.v2-channel-menu');
    return menu ? {
      parentTag: menu.parentElement?.tagName,
      actions: [...menu.querySelectorAll('[data-ch-action]')].map(b => b.getAttribute('data-ch-action')),
      boundChannel: menu.getAttribute('data-channel-id'),
    } : null;
  });
  check('menu opens on "…" click',                 menuState != null);
  check('menu is a body-level popover',            menuState?.parentTag === 'BODY');
  check('menu carries the row\'s channel id',      menuState?.boundChannel === chId);
  check('menu offers restart / stop / rename',
    JSON.stringify(menuState?.actions) === JSON.stringify(['restart', 'stop', 'rename']),
    JSON.stringify(menuState?.actions));

  const activeAfter = await page.evaluate(() =>
    ({ now: window.__v2debug.stores.uiStore.getActiveChannel(), start: window.__activeAtStart }));
  check('"…" click did NOT activate the channel',
    activeAfter.now === activeAfter.start,
    JSON.stringify(activeAfter));

  // 3. Restart fires intent.restart_agent for THIS channel and closes the menu.
  await page.evaluate(() => {
    window.__lastRestart = null;
    window.__v2debug.bus.on('intent.restart_agent', (p) => { window.__lastRestart = p; });
  });
  await page.click('[data-ch-action="restart"]');
  await page.waitForTimeout(80);
  const restarted = await page.evaluate(() => window.__lastRestart);
  check('Restart fires intent.restart_agent with the row\'s channelId',
    restarted?.channelId === chId, JSON.stringify(restarted));
  const gone = await page.$('.v2-channel-menu') == null;
  check('menu closes after action click', gone);

  // 4. Stop fires intent.stop_agent.
  await trigger.click({ force: true });
  await page.waitForTimeout(60);
  await page.evaluate(() => {
    window.__lastStop = null;
    window.__v2debug.bus.on('intent.stop_agent', (p) => { window.__lastStop = p; });
  });
  await page.click('[data-ch-action="stop"]');
  await page.waitForTimeout(80);
  const stopped = await page.evaluate(() => window.__lastStop);
  check('Stop fires intent.stop_agent', stopped?.channelId === chId, JSON.stringify(stopped));

  // 5. Rename. Activates the channel (to expose the overlay title),
  //    opens an inline input, Enter commits via intent.update_channel.
  await trigger.click({ force: true });
  await page.waitForTimeout(60);
  await page.evaluate(() => {
    window.__lastUpdate = null;
    window.__v2debug.bus.on('intent.update_channel', (p) => { window.__lastUpdate = p; });
  });
  await page.click('[data-ch-action="rename"]');
  await page.waitForTimeout(120);
  // Ensure overlay is open so the title + input are visible.
  if (!(await page.$eval('#v2-chat-overlay', el => el.classList.contains('open')))) {
    await page.click('#v2-rail-chat-toggle');
    await page.waitForTimeout(250);
  }
  await page.waitForSelector('.v2-co-rename-input', { timeout: 2000 });
  await page.locator('.v2-co-rename-input').fill('Renamed');
  await page.locator('.v2-co-rename-input').press('Enter');
  await page.waitForTimeout(120);
  const updated = await page.evaluate(() => window.__lastUpdate);
  check('rename commit fires intent.update_channel with name patch',
    updated?.channelId === chId && updated?.patch?.name === 'Renamed',
    JSON.stringify(updated));

  // 6. Esc on rename cancels cleanly — no update intent after.
  await trigger.click({ force: true });
  await page.waitForTimeout(60);
  await page.click('[data-ch-action="rename"]');
  await page.waitForSelector('.v2-co-rename-input', { timeout: 2000 });
  await page.evaluate(() => { window.__lastUpdate = null; });
  await page.locator('.v2-co-rename-input').fill('Nope');
  await page.locator('.v2-co-rename-input').press('Escape');
  await page.waitForTimeout(80);
  const cancel = await page.evaluate(() => window.__lastUpdate);
  check('Esc on rename cancels — no update intent', cancel === null);

  // 7. Outside click closes the menu.
  await trigger.click({ force: true });
  await page.waitForTimeout(60);
  await page.mouse.click(5, 5);
  await page.waitForTimeout(80);
  const gone2 = await page.$('.v2-channel-menu') == null;
  check('outside click closes the menu', gone2);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
