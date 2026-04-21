// Verify the git complication chip:
//   1. When upstream is tracked, ↑N / ↓N always render (even at 0).
//   2. Clicking the chip opens a popover menu with detail + actions.
//   3. Clicking an action button emits intent.resolve_complication.
//   4. Clicking outside closes the popover.
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await context.newPage();

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
  const chId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  await page.click(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
  await page.waitForTimeout(500);

  const COMP_ID = 'git:/tmp/test-repo';
  await page.evaluate(({ id, compId }) => {
    window.__v2debug.stores.complicationsStore.upsert(id, {
      id: compId,
      kind: 'git-status',
      timestamp: Date.now(),
      data: {
        repo: '/tmp/test-repo',
        branch: 'main',
        upstream: 'origin/main',
        ahead: 0,
        behind: 0,
        insertions: 3,
        deletions: 1,
        staged:   { added: 1, modified: 0, deleted: 0, total: 1 },
        unstaged: { added: 0, modified: 2, deleted: 0, total: 2 },
        untracked: 0,
        conflicts: 0,
        remote_name: 'owner/repo',
      },
      options: [
        { id: 'push',  label: 'Push',  enabled: true },
        { id: 'fetch', label: 'Fetch', enabled: true },
      ],
    });
  }, { id: chId, compId: COMP_ID });
  await page.waitForTimeout(150);

  // 1. Chip renders ahead/behind even at 0.
  const chipStats = await page.evaluate(() => {
    const el = document.querySelector('.v2-comp-git[data-comp-id="git:/tmp/test-repo"]');
    if (!el) return null;
    return {
      html: el.innerHTML,
      hasUpArrow: /↑0/.test(el.textContent),
      hasDownArrow: /↓0/.test(el.textContent),
      hasAdd: /\+3/.test(el.textContent),
      hasDel: /-1/.test(el.textContent),
    };
  });
  check('chip shows +3 insertions', chipStats?.hasAdd, JSON.stringify(chipStats?.hasAdd));
  check('chip shows -1 deletion',  chipStats?.hasDel, JSON.stringify(chipStats?.hasDel));
  check('chip shows ↑0 ahead even when 0', chipStats?.hasUpArrow);
  check('chip shows ↓0 behind even when 0', chipStats?.hasDownArrow);

  // 2. Click the chip → menu opens.
  // Hook the bus for intent.resolve_complication so we can assert later.
  await page.evaluate(() => {
    window.__lastIntent = null;
    window.__v2debug.bus.on('intent.resolve_complication', (p) => { window.__lastIntent = p; });
  });
  await page.click('.v2-comp-git[data-comp-id="git:/tmp/test-repo"]');
  await page.waitForTimeout(120);
  const menuState = await page.evaluate(() => {
    const menu = document.querySelector('.v2-comp-menu');
    return {
      present: !!menu,
      hasBranch:   !!menu && /Branch/.test(menu.textContent),
      hasStaged:   !!menu && /Staged/.test(menu.textContent),
      hasUnstaged: !!menu && /Unstaged/.test(menu.textContent),
      hasRemote:   !!menu && /Remote/.test(menu.textContent),
      actionCount: menu ? menu.querySelectorAll('[data-action]').length : 0,
    };
  });
  check('menu opens with Branch section',   menuState.hasBranch,   JSON.stringify(menuState));
  check('menu shows Staged',                menuState.hasStaged);
  check('menu shows Unstaged',              menuState.hasUnstaged);
  check('menu shows Remote (ahead/behind)', menuState.hasRemote);
  check('menu has 2 action buttons',        menuState.actionCount === 2, String(menuState.actionCount));

  // 3. Outside-click closes while menu is still open.
  // (Menu is currently open from the click at "2." above.)
  const viewport = page.viewportSize();
  await page.mouse.click(viewport.width - 20, 10);
  await page.waitForTimeout(150);
  const closed = await page.evaluate(() => !document.querySelector('.v2-comp-menu'));
  check('outside click closes the menu', closed);

  // 4. Re-open then click a button → intent fires, menu closes.
  await page.locator('.v2-comp-git[data-comp-id="git:/tmp/test-repo"]').click({ force: true });
  await page.waitForTimeout(120);
  const reopened = await page.evaluate(() => !!document.querySelector('.v2-comp-menu'));
  check('chip click re-opens menu', reopened);

  // Fire the button click directly in-page — bypasses Playwright's
  // coordinate-based dispatch and guarantees the click target is the
  // action button, not the surrounding chip.
  await page.evaluate(() => {
    const btn = document.querySelector(
      '.v2-comp-git[data-comp-id="git:/tmp/test-repo"] .v2-comp-menu [data-action="push"]'
    );
    btn?.click();
  });
  await page.waitForTimeout(150);
  const intent = await page.evaluate(() => window.__lastIntent);
  check('action click dispatches intent.resolve_complication',
    intent && intent.channelId === chId && intent.complicationId === COMP_ID && intent.action === 'push',
    JSON.stringify(intent));
  const closedAfterAction = await page.evaluate(() => !document.querySelector('.v2-comp-menu'));
  check('menu closes after action click', closedAfterAction);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
