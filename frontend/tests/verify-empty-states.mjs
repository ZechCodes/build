// Verify empty-state UX:
//   - viewer placeholder shows when no file is selected
//   - path bar is hidden on desktop while no file is selected
//   - mode bar stays hidden until a file is selected
//   - Modified tree shows the centered "no changes yet" placeholder
//     when the changes list is empty
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
  const chId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  await page.click(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
  await page.waitForTimeout(400);

  // Clear any auto-selected file so we can see the empty state. We:
  //   1. Flush the changes list so _renderTree's auto-select can't fire.
  //   2. Null the channel's filesPath.
  //   3. Manually call the files view's _renderViewer() since it only
  //      re-renders in response to store events of kinds we didn't
  //      trigger (read_result / diff_result).
  await page.evaluate(() => {
    const d = window.__v2debug;
    const chId = d.stores.uiStore.getActiveChannel();
    d.stores.filesStore.setChanges(chId, []);
    const ch = d.channelRegistry.pool.get(chId);
    if (ch?.viewState) ch.viewState.filesPath = null;
    // Walk active views to find the files-view (the one exposing
    // _renderViewer) and kick it.
    const chan = d.channelRegistry.active;
    if (chan?.views) {
      const filesView = chan.views.files;
      if (typeof filesView?._renderViewer === 'function') filesView._renderViewer();
    }
  });
  await page.waitForTimeout(200);

  const viewer = await page.evaluate(() => {
    const ph = document.querySelector('.v2-files-viewer-body .v2-files-placeholder');
    const modeBar = document.querySelector('.v2-files-mode-bar');
    const pathBar = document.getElementById('v2-path-bar');
    return {
      hasPlaceholder: !!ph,
      title: ph?.querySelector('.v2-files-placeholder-title')?.textContent?.trim(),
      body:  ph?.querySelector('.v2-files-placeholder-body')?.textContent?.trim(),
      iconPresent: !!ph?.querySelector('.v2-files-placeholder-icon'),
      modeBarHidden: modeBar?.hidden === true || getComputedStyle(modeBar).display === 'none',
      pathBarHidden: getComputedStyle(pathBar).display === 'none',
    };
  });
  check('viewer shows placeholder when no file selected', viewer.hasPlaceholder);
  check('placeholder has a title',                        !!viewer.title);
  check('placeholder has a body copy',                    !!viewer.body);
  check('placeholder has an icon',                        viewer.iconPresent);
  check('mode bar is hidden when no file selected',       viewer.modeBarHidden);
  check('path bar is hidden on desktop when no file selected',
        viewer.pathBarHidden);

  // Modified-tab empty state: force empty changes via the store.
  await page.evaluate(() => {
    const d = window.__v2debug;
    const chId = d.stores.uiStore.getActiveChannel();
    d.stores.filesStore.setChanges(chId, []);
  });
  await page.waitForTimeout(150);

  const tree = await page.evaluate(() => {
    const ph = document.querySelector('.v2-files-tree-body .v2-files-placeholder');
    return {
      present: !!ph,
      compact: ph?.classList.contains('v2-files-placeholder-compact'),
      title: ph?.querySelector('.v2-files-placeholder-title')?.textContent?.trim(),
      body:  ph?.querySelector('.v2-files-placeholder-body')?.textContent?.trim(),
    };
  });
  check('Modified tree shows placeholder when empty',  tree.present);
  check('Modified tree uses the compact variant',      tree.compact);
  check('Modified tree title is non-empty',            !!tree.title);
  check('Modified tree body references edits or changes',
        /(change|edit)/i.test(tree.body || ''), tree.body);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
