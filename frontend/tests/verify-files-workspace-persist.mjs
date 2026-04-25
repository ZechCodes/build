// Verify the files-view workspace state (active tab, selected file,
// expanded dirs, scroll position) is restored when the user comes
// back to the channel — both from a channel switch and from a full
// page reload.
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

async function seedTree(page, chId) {
  // Force the All tab then populate a synthetic tree so the test
  // doesn't depend on the device's real files.
  await page.evaluate((id) => {
    const s = window.__v2debug.stores.filesStore;
    s.setTree(id, '', { entries: [
      { name: 'api',   type: 'dir'  },
      { name: 'cache', type: 'dir'  },
      { name: 'docs',  type: 'dir'  },
    ], truncated: false });
    s.setTree(id, 'api', { entries: [
      { name: 'models.py', type: 'file' },
      { name: 'views.py',  type: 'file' },
    ], truncated: false });
    // Seed a read result so the viewer paints real content.
    s.setReadResult(id, {
      channel_id: id,
      path: 'api/models.py',
      content: Array.from({ length: 400 }, (_, i) => `line ${i + 1}`).join('\n'),
      is_image: false,
    });
  }, chId);
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

  // Find two channels so we can switch between them.
  const chIds = await page.$$eval('.v2-channel-sidebar-item', els =>
    els.map(el => el.getAttribute('data-channel-id')).filter(Boolean)
  );
  const idA = chIds[0];
  // Clear any prior persisted state for determinism.
  await page.evaluate((ids) => {
    for (const id of ids) localStorage.removeItem(`v2:channel:${id}:viewState`);
  }, chIds);

  // Activate channel A.
  await page.click(`.v2-channel-sidebar-item[data-channel-id="${idA}"]`);
  await page.waitForTimeout(400);

  // Build the workspace: switch to All tab, expand api/, select models.py,
  // scroll viewer down.
  await seedTree(page, idA);
  await page.locator('[data-tree-tab="all"]').click();
  await page.waitForTimeout(80);
  await page.locator('[data-dir-path="api"]').click();
  await page.waitForTimeout(80);
  await page.locator('[data-file-path="api/models.py"]').click();
  await page.waitForTimeout(120);
  // Clicking the file kicked intent.file_read, which may overwrite our
  // seeded readResult with a real fetch (or an error). Reseed explicitly
  // so the viewer has 400 lines of deterministic content to scroll.
  await seedTree(page, idA);
  // Force the read result to be paired with the selected path so the
  // viewer paints it.
  await page.evaluate((id) => {
    window.__v2debug.stores.filesStore.setReadResult(id, {
      channel_id: id,
      path: 'api/models.py',
      content: Array.from({ length: 400 }, (_, i) => `line ${i + 1}`).join('\n'),
      is_image: false,
    });
  }, idA);
  await page.waitForTimeout(80);

  // Scroll the viewer panel and let the scroll event run through our
  // persist listener.
  await page.evaluate(() => {
    const v = document.querySelector('.v2-files-viewer-body');
    if (v) v.scrollTop = 350;
    // Some browsers skip the scroll event on same-tick programmatic
    // scrollTop assignments; dispatch one explicitly for safety.
    v?.dispatchEvent(new Event('scroll'));
  });
  await page.waitForTimeout(100);

  // 1. Switch channels + come back → workspace intact (in-memory pool).
  if (chIds.length > 1) {
    await page.click(`.v2-channel-sidebar-item[data-channel-id="${chIds[1]}"]`);
    await page.waitForTimeout(250);
    await page.click(`.v2-channel-sidebar-item[data-channel-id="${idA}"]`);
    await page.waitForTimeout(400);

    const afterSwitch = await page.evaluate(() => ({
      tab:         document.querySelector('[data-tree-tab="all"]').classList.contains('active'),
      apiExpanded: !!document.querySelector('.v2-files-level [data-file-path="api/models.py"]'),
      selected:    !!document.querySelector('[data-file-path="api/models.py"].active'),
      viewerScroll: document.querySelector('.v2-files-viewer-body')?.scrollTop || 0,
    }));
    check('All tab stays active after switch-back',        afterSwitch.tab === true, JSON.stringify(afterSwitch));
    check('api/ stays expanded after switch-back',         afterSwitch.apiExpanded === true);
    check('models.py stays selected after switch-back',    afterSwitch.selected === true);
    check('viewer scroll restored after switch-back',
      Math.abs(afterSwitch.viewerScroll - 350) < 5, `scroll=${afterSwitch.viewerScroll}`);
  } else {
    console.log('[SKIP] switch-back coverage — only one channel on this device');
  }

  // 2. Full page reload → localStorage restores same workspace.
  // Fire pagehide manually since Playwright's reload triggers unload.
  await page.evaluate(() => window.dispatchEvent(new Event('pagehide')));
  await page.waitForTimeout(50);
  await page.reload({ waitUntil: 'domcontentloaded' });
  await page.waitForSelector(`.v2-channel-sidebar-item[data-channel-id="${idA}"]`, { timeout: 8000 });
  // Activate the same channel (may not be auto-activated after reload).
  const stillActive = await page.evaluate((id) => {
    return window.__v2debug.stores.uiStore.getActiveChannel() === id;
  }, idA);
  if (!stillActive) {
    await page.click(`.v2-channel-sidebar-item[data-channel-id="${idA}"]`);
  }
  await page.waitForTimeout(400);
  // Reseed the tree data — it's not persisted server-side, but the
  // expanded-dirs list + filesPath should be. Once we reseed, _renderTree
  // uses expandedDirs to show api/'s children without having to click.
  await seedTree(page, idA);
  await page.waitForTimeout(120);

  const vs = await page.evaluate((id) => {
    return window.__v2debug.channelRegistry.pool.get(id)?.viewState;
  }, idA);
  check('localStorage restored filesPath',
    vs?.filesPath === 'api/models.py', vs?.filesPath);
  check('localStorage restored filesTreeTab="all"',
    vs?.filesTreeTab === 'all', vs?.filesTreeTab);
  check('localStorage restored expandedDirs includes api',
    Array.isArray(vs?.filesExpandedDirs) && vs.filesExpandedDirs.includes('api'),
    JSON.stringify(vs?.filesExpandedDirs));
  check('localStorage restored viewer scrollTop',
    Math.abs((vs?.filesViewerScrollTop || 0) - 350) < 5, String(vs?.filesViewerScrollTop));

  // And the DOM actually reflects it (All tab active, api/ expanded).
  const afterReload = await page.evaluate(() => ({
    tab:         document.querySelector('[data-tree-tab="all"]').classList.contains('active'),
    apiExpanded: !!document.querySelector('.v2-files-level [data-file-path="api/models.py"]'),
  }));
  check('after reload, All tab is active',  afterReload.tab === true, JSON.stringify(afterReload));
  check('after reload, api/ is expanded',   afterReload.apiExpanded === true);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
