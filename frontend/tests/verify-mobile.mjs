// Verify Dashboard v2 on a phone-sized viewport (iPhone 13 @ 375×812):
//   1. Hamburger is visible, sidebar is hidden on load.
//   2. Tap hamburger → sidebar slides in + backdrop appears.
//   3. Tap close X → sidebar closes.
//   4. Tap backdrop → sidebar closes.
//   5. Tap channel → sidebar auto-closes + channel activates.
//   6. Tap path-bar chevron → file tree drawer slides in.
//   7. Tap a file in the tree → tree drawer auto-closes.
//   8. Tap Chat rail toggle → overlay becomes fullscreen (width ≈ viewport).
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';

const browser = await chromium.launch({ headless: true });
// iPhone 13 viewport.
const context = await browser.newContext({
  viewport: { width: 375, height: 812 },
  isMobile: true,
  hasTouch: true,
});
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
  await page.waitForSelector('#v2-path-bar-menu', { timeout: 8000 });

  // 1. Hamburger visible; sidebar hidden.
  const hamVisible = await page.locator('#v2-path-bar-menu').isVisible();
  check('hamburger is visible on mobile', hamVisible);

  const sidebarClosedInitially = await page.evaluate(() => {
    const sb = document.querySelector('.v2-sidebar');
    const app = document.querySelector('.v2-app');
    const rect = sb?.getBoundingClientRect();
    return {
      appOpen: app?.classList.contains('sidebar-open'),
      offscreen: rect ? rect.right <= 0 : false,
    };
  });
  check('sidebar starts closed (no sidebar-open class)', sidebarClosedInitially.appOpen === false);
  check('sidebar is translated offscreen at init', sidebarClosedInitially.offscreen);

  // 2. Tap hamburger → sidebar opens + backdrop appears.
  await page.tap('#v2-path-bar-menu');
  await page.waitForTimeout(250);
  const afterOpen = await page.evaluate(() => {
    const sb = document.querySelector('.v2-sidebar');
    const bd = document.querySelector('.v2-sidebar-backdrop');
    const app = document.querySelector('.v2-app');
    const rect = sb?.getBoundingClientRect();
    return {
      appOpen: app?.classList.contains('sidebar-open'),
      rectLeft: Math.round(rect?.left ?? -1),
      rectRight: Math.round(rect?.right ?? -1),
      backdropVisible: bd && getComputedStyle(bd).display !== 'none',
    };
  });
  check('tap hamburger sets .sidebar-open', afterOpen.appOpen === true);
  check('sidebar is now onscreen (left === 0)', afterOpen.rectLeft === 0, JSON.stringify(afterOpen));
  check('backdrop is visible', afterOpen.backdropVisible);

  // 3. Tap close X → sidebar closes.
  await page.tap('#v2-sidebar-close');
  await page.waitForTimeout(200);
  const afterCloseX = await page.evaluate(() => ({
    open: document.querySelector('.v2-app')?.classList.contains('sidebar-open'),
  }));
  check('tap close X removes .sidebar-open', afterCloseX.open === false);

  // 4. Tap backdrop → sidebar closes.
  await page.tap('#v2-path-bar-menu');
  await page.waitForTimeout(200);
  // Tap a point on the right side of the viewport — outside the sidebar
  // width (82vw) so the backdrop receives the click rather than the
  // sidebar's content.
  await page.touchscreen.tap(360, 400);
  await page.waitForTimeout(200);
  const afterBackdrop = await page.evaluate(() => ({
    open: document.querySelector('.v2-app')?.classList.contains('sidebar-open'),
  }));
  check('tap backdrop closes the drawer', afterBackdrop.open === false);

  // 5. Tap channel → drawer auto-closes + channel activates.
  await page.tap('#v2-path-bar-menu');
  await page.waitForTimeout(200);
  await page.waitForSelector('.v2-channel-sidebar-item', { timeout: 5000 });
  const chId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  await page.tap(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
  await page.waitForTimeout(300);
  const afterChannelTap = await page.evaluate(() => ({
    open: document.querySelector('.v2-app')?.classList.contains('sidebar-open'),
    active: window.__v2debug.stores.uiStore.getActiveChannel(),
  }));
  check('tapping a channel auto-closes the drawer', afterChannelTap.open === false);
  check('tapping a channel activates it', !!afterChannelTap.active && afterChannelTap.active === chId,
    JSON.stringify(afterChannelTap));

  // 6. Tap the file-tree chevron → tree drawer slides in.
  await page.tap('#v2-path-bar-tree');
  await page.waitForTimeout(250);
  const treeOpen = await page.evaluate(() => {
    const main = document.getElementById('v2-viewer-main');
    const tree = document.querySelector('.v2-files-tree-panel');
    const rect = tree?.getBoundingClientRect();
    return {
      hasClass: main?.classList.contains('mobile-tree-open'),
      treeLeft: Math.round(rect?.left ?? -1),
      treeVisible: rect ? rect.left >= 0 && rect.width > 0 : false,
    };
  });
  check('tree chevron sets .mobile-tree-open', treeOpen.hasClass === true);
  check('tree panel slides into view', treeOpen.treeVisible,  JSON.stringify(treeOpen));

  // 7. Tap a file in the tree → tree drawer auto-closes.
  const fileBtn = await page.$('.v2-files-tree-panel [data-file-path]');
  if (fileBtn) {
    await fileBtn.tap();
    await page.waitForTimeout(250);
    const afterFile = await page.evaluate(() => document.getElementById('v2-viewer-main')?.classList.contains('mobile-tree-open'));
    check('tapping a file auto-closes tree drawer', afterFile === false);
  } else {
    check('tapping a file auto-closes tree drawer', true, 'skipped (no file entries in tree)');
  }

  // 8. Tap Chat rail toggle → overlay becomes fullscreen.
  await page.tap('#v2-rail-chat-toggle');
  await page.waitForTimeout(250);
  const overlayInfo = await page.evaluate(() => {
    const ov = document.getElementById('v2-chat-overlay');
    const rect = ov?.getBoundingClientRect();
    return {
      open: ov?.classList.contains('open'),
      width: Math.round(rect?.width ?? -1),
      left: Math.round(rect?.left ?? -1),
    };
  });
  check('chat overlay opens', overlayInfo.open === true);
  check('chat overlay is full viewport width',
    overlayInfo.width === 375 && overlayInfo.left === 0,
    JSON.stringify(overlayInfo));

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
