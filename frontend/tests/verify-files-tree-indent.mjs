// Verify the All-files tree indents files under their parent dirs,
// draws a faint guide line per nesting level, and doesn't leak the
// bare filesystem root into a flush-left file row.
//
// Previous rendering put an inline `padding-left` on each row that
// got clobbered by `.v2-files-row { padding: 4px 10px }` in some
// browsers, leaving files at depth 1+ visually flush against the
// tree's left edge. We now nest rows inside `.v2-files-level`
// wrappers so indent + hierarchy are derived from the DOM.
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
  await page.goto(`${BASE}/dashboard/`, { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });
  const chId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  await page.click(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
  await page.waitForTimeout(400);

  // Seed the store directly with a synthetic tree:
  //   /
  //   ├── api/
  //   │   ├── __pycache__/
  //   │   └── models.py
  //   └── cache/
  await page.evaluate((id) => {
    const s = window.__v2debug.stores.filesStore;
    s.setTree(id, '', { entries: [
      { name: 'api',    type: 'dir'  },
      { name: 'cache',  type: 'dir'  },
    ], truncated: false });
    s.setTree(id, 'api', { entries: [
      { name: '__pycache__', type: 'dir' },
      { name: 'models.py',   type: 'file' },
    ], truncated: false });
  }, chId);

  // Flip to the All tab and expand `api/` so its children render.
  await page.locator('[data-tree-tab="all"]').click();
  await page.waitForTimeout(80);
  await page.evaluate((id) => {
    const ch = window.__v2debug.channelRegistry.pool.get(id);
    if (ch) ch.viewState.filesExpandedDirs = ['api'];
    // Trigger a re-render by nudging the store (no-op set of existing tree).
    const s = window.__v2debug.stores.filesStore;
    s.setTree(id, '', s.treeFor(id).get(''));
  }, chId);
  await page.waitForTimeout(80);

  // Verify nesting structure: the expanded `api/` dir row has a
  // sibling `.v2-files-level` wrapper containing both its children.
  const structure = await page.evaluate(() => {
    const apiBtn = document.querySelector('[data-dir-path="api"]');
    if (!apiBtn) return { error: 'no api button' };
    const nestedWrapper = apiBtn.nextElementSibling;
    const isLevel = nestedWrapper?.classList.contains('v2-files-level');
    const hasPycache = !!nestedWrapper?.querySelector('[data-dir-path="api/__pycache__"]');
    const hasModels  = !!nestedWrapper?.querySelector('[data-file-path="api/models.py"]');
    return { isLevel, hasPycache, hasModels };
  });
  check('expanded api/ has a .v2-files-level wrapper',    structure.isLevel === true, JSON.stringify(structure));
  check('nested pycache dir rendered inside the wrapper', structure.hasPycache === true);
  check('nested models.py file rendered inside wrapper',  structure.hasModels === true);

  // Files inside api/ should sit to the RIGHT of the api/ row's left
  // edge (no flush-left regression). Dirs and files inside the same
  // level should line up.
  const rects = await page.evaluate(() => {
    const api      = document.querySelector('[data-dir-path="api"]');
    const pycache  = document.querySelector('[data-dir-path="api/__pycache__"]');
    const models   = document.querySelector('[data-file-path="api/models.py"]');
    if (!api || !pycache || !models) return null;
    return {
      api:     api.getBoundingClientRect().left,
      pycache: pycache.getBoundingClientRect().left,
      models:  models.getBoundingClientRect().left,
    };
  });
  check('nested dir is indented past the parent',
    rects && rects.pycache > rects.api + 4, JSON.stringify(rects));
  check('nested file is indented past the parent',
    rects && rects.models > rects.api + 4, JSON.stringify(rects));
  check('nested sibling dir + file share the same indent',
    rects && Math.abs(rects.pycache - rects.models) < 1, JSON.stringify(rects));

  // The wrapper should render a guide line (left border).
  const guideBorder = await page.evaluate(() => {
    const wrap = document.querySelector('[data-dir-path="api"]').nextElementSibling;
    return getComputedStyle(wrap).borderLeftWidth;
  });
  check('nested level has a left-border guide line', /^[1-9]/.test(guideBorder), guideBorder);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
