// Verify drag-and-drop + per-dir upload buttons wire file uploads
// into the correct destination directory.
//
// Rather than run a real E2EE upload (requires an online bridge and
// a real workspace on the remote side), we stub the FilesView's
// `_dispatchUpload` method on the active channel's view instance
// and assert which (file, destDir) arguments it received.
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
  await page.waitForTimeout(400);

  // Stub FilesView._dispatchUpload so we can assert its arguments.
  await page.evaluate((id) => {
    const ch = window.__v2debug.channelRegistry.pool.get(id);
    const files = ch?.views?.files;
    window.__uploadCalls = [];
    files._dispatchUpload = async (file, destDir) => {
      window.__uploadCalls.push({
        name: file?.name || null,
        size: file?.size || 0,
        destDir: destDir || '',
      });
    };
  }, chId);

  // Seed a small tree so the per-dir upload button renders.
  await page.evaluate((id) => {
    const s = window.__v2debug.stores.filesStore;
    s.setTree(id, '', { entries: [
      { name: 'api',   type: 'dir'  },
      { name: 'docs',  type: 'dir'  },
    ], truncated: false });
    const ch = window.__v2debug.channelRegistry.pool.get(id);
    ch.viewState.filesExpandedDirs = [];
  }, chId);
  await page.locator('[data-tree-tab="all"]').click();
  await page.waitForTimeout(80);

  // 1. Tree tabs header has an Upload button targeting the root.
  const rootBtn = await page.locator('.v2-files-tree-upload[data-dir-upload=""]');
  check('tree header has a root-upload button', await rootBtn.count() === 1);

  // 2. Hidden file input exists.
  const hiddenInputCount = await page.locator('.v2-files-file-input[type="file"]').count();
  check('hidden <input type=file> rendered in tree panel', hiddenInputCount === 1);

  // 3. Each dir row has a nested upload button with the dir's path.
  const apiUploadBtn = await page.locator('[data-dir-upload="api"]');
  check('dir row renders its own upload trigger', await apiUploadBtn.count() >= 1);

  // 4. Clicking a dir-row upload icon stamps the destDir onto the
  //    hidden input AND triggers a click on it (native picker opens).
  //    We intercept the click on the input to capture it without the
  //    picker actually opening in a headless browser.
  await page.evaluate(() => {
    const inp = document.querySelector('.v2-files-file-input');
    window.__inputClicks = 0;
    inp.addEventListener('click', (e) => {
      e.preventDefault();                // block the native picker
      window.__inputClicks++;
    });
  });
  await page.locator('[data-dir-upload="api"]').first().click();
  await page.waitForTimeout(50);
  const pickedState = await page.evaluate(() => ({
    clicks: window.__inputClicks,
    dest:   document.querySelector('.v2-files-file-input').dataset.destDir,
  }));
  check('clicking upload-on-dir stamps destDir + opens picker',
    pickedState.clicks === 1 && pickedState.dest === 'api',
    JSON.stringify(pickedState));

  // 5. Clicking the upload trigger does NOT expand/collapse the dir
  //    (click was stopped at the upload-trigger branch).
  const apiExpanded = await page.evaluate((id) => {
    const ch = window.__v2debug.channelRegistry.pool.get(id);
    return ch.viewState.filesExpandedDirs.includes('api');
  }, chId);
  check('upload click does NOT toggle the dir expansion', apiExpanded === false);

  // 6. Drop handler: synthesize a drop on the `docs` dir row with a
  //    fake File, confirm _dispatchUpload got (file, 'docs').
  const fileBlob = 'hello';
  await page.evaluate((blob) => {
    const row = document.querySelector('[data-dir-path="docs"]');
    const file = new File([blob], 'notes.md', { type: 'text/markdown' });
    const dt = new DataTransfer();
    dt.items.add(file);
    row.dispatchEvent(new DragEvent('dragenter', { bubbles: true, cancelable: true, dataTransfer: dt }));
    row.dispatchEvent(new DragEvent('dragover',  { bubbles: true, cancelable: true, dataTransfer: dt }));
    row.dispatchEvent(new DragEvent('drop',      { bubbles: true, cancelable: true, dataTransfer: dt }));
  }, fileBlob);
  await page.waitForTimeout(80);
  const calls = await page.evaluate(() => window.__uploadCalls);
  check('drop on docs/ dispatches upload with destDir="docs"',
    calls.some(c => c.name === 'notes.md' && c.destDir === 'docs'),
    JSON.stringify(calls));

  // 7. Drop outside any dir row (on the tree body itself) uploads to
  //    the root (destDir === '').
  await page.evaluate(() => { window.__uploadCalls = []; });
  await page.evaluate(() => {
    const body = document.querySelector('.v2-files-tree-body');
    const file = new File(['a'], 'root.txt', { type: 'text/plain' });
    const dt = new DataTransfer();
    dt.items.add(file);
    body.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: dt }));
  });
  await page.waitForTimeout(80);
  const rootCalls = await page.evaluate(() => window.__uploadCalls);
  check('drop on empty tree area targets repo root',
    rootCalls.some(c => c.name === 'root.txt' && c.destDir === ''),
    JSON.stringify(rootCalls));

  // 8. Progress chip appears on `upload.progress` and disappears on
  //    `upload.done`.
  await page.evaluate(() => {
    window.__v2debug.bus.emit('upload.progress', {
      file_id: 'test-1', filename: 'big.bin', progress: 0.42,
    });
  });
  await page.waitForTimeout(60);
  const chipShown = await page.evaluate(() => {
    const chip = document.querySelector('.v2-files-upload-chip[data-file-id="test-1"]');
    return chip ? {
      name: chip.querySelector('.v2-files-upload-name')?.textContent,
      pct:  chip.querySelector('.v2-files-upload-pct')?.textContent,
    } : null;
  });
  check('progress chip renders on upload.progress',
    chipShown?.name === 'big.bin' && chipShown?.pct === '42%', JSON.stringify(chipShown));

  await page.evaluate(() => {
    window.__v2debug.bus.emit('upload.done', { file_id: 'test-1' });
  });
  await page.waitForTimeout(60);
  const chipGone = await page.evaluate(() =>
    !document.querySelector('.v2-files-upload-chip[data-file-id="test-1"]'));
  check('progress chip clears on upload.done', chipGone);

  // 9. drag-active class flips on dragenter with Files.
  await page.evaluate(() => { window.__v2debug.stores.uiStore.getActiveChannel(); });
  await page.evaluate(() => {
    const panel = document.getElementById('v2-files-tree-panel');
    const dt = new DataTransfer();
    const f = new File(['x'], 'x.txt', { type: 'text/plain' });
    dt.items.add(f);
    panel.dispatchEvent(new DragEvent('dragenter', { bubbles: true, cancelable: true, dataTransfer: dt }));
  });
  await page.waitForTimeout(30);
  const dragActive = await page.evaluate(() =>
    document.getElementById('v2-files-tree-panel').classList.contains('drag-active'));
  check('tree panel flips .drag-active on file dragenter', dragActive);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
