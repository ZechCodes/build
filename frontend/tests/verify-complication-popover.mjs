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
  await page.goto(`${BASE}/dashboard/`, { waitUntil: 'domcontentloaded' });
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
        // Bridge sends last_fetch as unix ms (st_mtime * 1000).
        last_fetch: Date.now() - 180_000,  // 3 minutes ago
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

  // The menu is portaled to <body> and positioned via fixed coords
  // computed from the chip's rect. It must NOT be clipped by the rail
  // (overflow: hidden) nor the scrolling strip (overflow-x: auto). Assert
  // the menu's rect sits fully within the viewport and its top is above
  // the chip (we anchor above, falling back to below only when no room).
  const geom = await page.evaluate(() => {
    const menu = document.querySelector('.v2-comp-menu');
    const chip = document.querySelector('.v2-comp-git[data-comp-id="git:/tmp/test-repo"]');
    if (!menu || !chip) return null;
    const m = menu.getBoundingClientRect();
    const c = chip.getBoundingClientRect();
    return {
      menuTop: m.top, menuLeft: m.left, menuRight: m.right, menuBottom: m.bottom,
      menuWidth: m.width, menuHeight: m.height,
      chipTop: c.top, chipBottom: c.bottom,
      viewW: window.innerWidth, viewH: window.innerHeight,
      position: getComputedStyle(menu).position,
      parentTag: menu.parentElement?.tagName,
    };
  });
  check('menu is a body-level fixed element',
    geom?.position === 'fixed' && geom?.parentTag === 'BODY', JSON.stringify(geom));
  check('menu fits inside the viewport (not clipped)',
    geom && geom.menuTop >= 0 && geom.menuLeft >= 0
        && geom.menuRight  <= geom.viewW
        && geom.menuBottom <= geom.viewH,
    JSON.stringify(geom));
  check('menu anchors above the chip (or below if no room above)',
    geom && (geom.menuBottom <= geom.chipTop || geom.menuTop >= geom.chipBottom),
    JSON.stringify(geom));

  // Last fetch arithmetic: bridge sends ms, code must treat it as ms.
  // 3 minutes ago → "3m ago" (never a negative number).
  const lastFetchText = await page.evaluate(() => {
    const menu = document.querySelector('.v2-comp-menu');
    const sections = menu ? [...menu.querySelectorAll('.v2-comp-menu-section')] : [];
    const section = sections.find(s => /Last fetch/i.test(s.textContent));
    return section ? section.querySelector('.v2-comp-menu-row')?.textContent?.trim() : null;
  });
  check('last_fetch renders a positive "Nm ago" label',
    lastFetchText && /^\d+m ago$/.test(lastFetchText),
    `label=${lastFetchText}`);

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
    // Menu is portaled to <body>, so query it directly, not as a
    // descendant of the chip.
    const btn = document.querySelector('.v2-comp-menu [data-action="push"]');
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
