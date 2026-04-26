// Verify Files view: All tab populates (dir vs file rows), Modified
// renders dir groups + commit-refresh on complications.bulk.

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
  await page.waitForSelector('.v2-channel-sidebar-item');
  await page.click('.v2-channel-sidebar-item');
  await page.waitForTimeout(1500);

  // Intercept send() for commit-refresh assert.
  await page.evaluate(() => {
    window.__sent = [];
    const pool = window.__v2debug.e2eePool;
    for (const inst of pool._instances.values()) {
      const orig = inst.send.bind(inst);
      inst.send = (payload) => { window.__sent.push(payload); return orig(payload); };
    }
  });

  // Click All tab.
  await page.click('[data-tree-tab="all"]');
  await page.waitForTimeout(400);

  const allCounts = await page.evaluate(() => ({
    dirRows: document.querySelectorAll('.v2-files-tree-body [data-dir-path]').length,
    fileRows: document.querySelectorAll('.v2-files-tree-body [data-file-path]').length,
  }));
  check('All tab populates with rows', allCounts.dirRows + allCounts.fileRows > 0,
    JSON.stringify(allCounts));
  check('All tab distinguishes dirs vs files', allCounts.dirRows > 0 || allCounts.fileRows > 0);

  // Click back to Modified — should show dir-group headers when files nest.
  await page.click('[data-tree-tab="changes"]');
  await page.waitForTimeout(300);

  // Inject synthetic nested changes.
  await page.evaluate((id) => {
    window.__v2debug.stores.filesStore.setChanges(id, [{
      branch: 'main',
      path: '.',
      entries: [
        { path: 'README.md', git_status: 'M', insertions: 3, deletions: 1 },
        { path: 'src/foo/a.js', git_status: 'M', insertions: 5, deletions: 2 },
        { path: 'src/foo/b.js', git_status: '?', insertions: 0, deletions: 0 },
      ],
    }, {
      branch: 'dev',
      path: 'packages/api',
      entries: [
        { path: 'packages/api/server.js', git_status: 'M', insertions: 8, deletions: 2 },
      ],
    }]);
    window.__v2debug.stores.filesStore.setCommits(id, '.', {
      commits: [
        { sha: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', short_sha: 'aaaaaaa', subject: 'latest' },
        { sha: 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', short_sha: 'bbbbbbb', subject: 'older' },
      ],
    });
    window.__v2debug.stores.filesStore.setCommits(id, 'packages/api', {
      commits: [
        { sha: 'cccccccccccccccccccccccccccccccccccccccc', short_sha: 'ccccccc', subject: 'api' },
      ],
    });
  }, await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id')));
  await page.waitForTimeout(200);

  const repoPanels = await page.$$eval('.v2-files-repo', els => els.map(e => e.getAttribute('data-repo')));
  check('Modified renders one boxed panel per repo',
    repoPanels.includes('.') && repoPanels.includes('packages/api'),
    repoPanels.join(' | '));

  const selectorCount = await page.$$eval('.v2-files-rev-controls select', els => els.length);
  check('repo revision selectors render below repo headers', selectorCount === 4, `count=${selectorCount}`);

  await page.click('[data-repo-toggle="packages/api"]');
  await page.waitForTimeout(100);
  const collapsed = await page.$eval('[data-repo="packages/api"]', el => el.classList.contains('collapsed'));
  check('repo header collapses its repo panel', collapsed);
  await page.click('[data-repo-toggle="packages/api"]');
  await page.waitForTimeout(100);

  const dirHeaders = await page.$$eval('.v2-files-dir-header', els => els.map(e => e.textContent.trim()));
  check('dir-group headers render in Modified', dirHeaders.includes('src/foo/'),
    dirHeaders.join(' | '));

  const filenames = await page.$$eval('.v2-files-tree-body .v2-files-name',
    els => els.map(e => e.textContent.trim()));
  check('file rows show basename (not full path)',
    filenames.includes('a.js') && filenames.includes('README.md') && !filenames.includes('src/foo/a.js'),
    filenames.join(' | '));

  await page.selectOption('[data-rev-menu="newer"][data-repo-path="."]', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa');
  await page.waitForTimeout(100);
  const revisionPayload = await page.evaluate(() => window.__sent.find(p => p.action === 'files_changes' && p.repo_path === '.'));
  check('changing revision menu sends scoped files_changes',
    revisionPayload?.newer_ref === 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' && revisionPayload?.older_ref === 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
    JSON.stringify(revisionPayload || null));

  // Commit-refresh: emit complications.bulk → expect files_changes intent.
  await page.evaluate(() => { window.__sent.length = 0; });
  await page.evaluate((id) => {
    window.__v2debug.bus.emit('complications.bulk', {
      channelId: id, complications: [{ id: 'g1', kind: 'git-status', data: { branch: 'main' } }],
    });
  }, await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id')));
  await page.waitForTimeout(400);
  const changes = await page.evaluate(() => window.__sent.filter(p => p.action === 'files_changes').length);
  check('complications.bulk triggers files_changes refresh', changes > 0, `count=${changes}`);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
