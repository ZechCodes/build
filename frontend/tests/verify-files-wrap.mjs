// Verify the file viewer Wrap toggle affects source and diff views.
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8092';
const EMAIL = process.env.EMAIL || 'files-wrap@test.local';

const browser = await chromium.launch({ headless: true });
const ctx = await browser.newContext({ viewport: { width: 1280, height: 820 } });
const page = await ctx.newPage();

let passed = 0, failed = 0;
function check(label, ok, note) {
  if (ok) { passed++; console.log(`[PASS] ${label}${note ? ` - ${note}` : ''}`); }
  else    { failed++; console.log(`[FAIL] ${label}${note ? ` - ${note}` : ''}`); }
}

try {
  await page.goto(`${BASE}/auth/dummy/login`, { waitUntil: 'domcontentloaded' });
  await page.fill('input[name="email"]', EMAIL);
  const name = await page.$('input[name="name"]');
  if (name) await page.fill('input[name="name"]', 'Files Wrap');
  await Promise.all([
    page.waitForURL(u => !u.pathname.startsWith('/auth/'), { timeout: 10000 }).catch(() => null),
    page.click('button[type="submit"]'),
  ]);

  await page.goto(`${BASE}/dashboard/`, { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.v2-app', { timeout: 10000 });
  await page.evaluate(() => {
    const d = window.__v2debug;
    d.bus.on('intent.get_messages', p => {
      if (p.channelId === 'ch-files-wrap') d.bus.emit('message.bulk', { channelId: 'ch-files-wrap', msgs: [] });
    });
    d.bus.on('intent.files_list', p => {
      if (p.channelId !== 'ch-files-wrap') return;
      d.bus.emit('files.list_result', {
        channelId: 'ch-files-wrap',
        path: p.path || '',
        entries: [{ name: 'wrap.txt', type: 'file' }],
      });
    });
    d.bus.on('intent.files_changes', p => {
      if (p.channelId === 'ch-files-wrap') d.bus.emit('files.changes_result', { channelId: 'ch-files-wrap', repos: [] });
    });
    d.bus.on('intent.file_read', p => {
      if (p.channelId !== 'ch-files-wrap') return;
      d.bus.emit('files.read_result', {
        channel_id: 'ch-files-wrap',
        path: p.path,
        content: 'const value = "' + Array.from({ length: 90 }, (_, i) => `word${i}`).join(' ') + '";',
        size: 1024,
        truncated: false,
      });
    });
    d.bus.on('intent.file_diff', p => {
      if (p.channelId !== 'ch-files-wrap') return;
      d.bus.emit('files.diff_result', {
        channel_id: 'ch-files-wrap',
        path: p.path,
        diff: [
          'diff --git a/wrap.txt b/wrap.txt',
          '--- a/wrap.txt',
          '+++ b/wrap.txt',
          '@@ -1 +1 @@',
          '-const value = "short";',
          '+const value = "' + Array.from({ length: 90 }, (_, i) => `word${i}`).join(' ') + '";',
        ].join('\n'),
      });
    });
    d.bus.emit('device.bulk', {
      devices: [{ id: 'dev-files-wrap', name: 'Files Device', status: 'online', has_transport_key: true }],
    });
    d.bus.emit('channel.list', {
      deviceId: 'dev-files-wrap',
      channels: [{ id: 'ch-files-wrap', name: 'files-wrap', created_at: Date.now() }],
    });
    d.bus.emit('sse.connected', {});
    d.bus.emit('e2ee.connected', { deviceId: 'dev-files-wrap' });
  });

  await page.waitForSelector('.v2-channel-sidebar-item[data-channel-id="ch-files-wrap"]', { timeout: 5000 });
  await page.click('.v2-channel-sidebar-item[data-channel-id="ch-files-wrap"]');
  await page.waitForFunction(() => window.__v2debug.channelRegistry.active?.id === 'ch-files-wrap');
  await page.evaluate(() => {
    window.__v2debug.channelRegistry.active.viewState.filesTreeTab = 'all';
    window.__v2debug.bus.emit('files.list_result', {
      channelId: 'ch-files-wrap',
      path: '',
      entries: [{ name: 'wrap.txt', type: 'file' }],
    });
  });
  await page.waitForSelector('.v2-files-row[data-file-path="wrap.txt"]', { timeout: 5000 });
  await page.click('.v2-files-row[data-file-path="wrap.txt"]');
  await page.waitForSelector('.v2-src-text', { timeout: 5000 });

  const before = await page.evaluate(() => {
    const text = document.querySelector('.v2-src-text');
    return {
      hasWrapClass: !!document.querySelector('.v2-src.wrap'),
      whiteSpace: getComputedStyle(text).whiteSpace,
      height: text.getBoundingClientRect().height,
    };
  });
  check('source starts unwrapped', before.hasWrapClass === false && before.whiteSpace === 'pre', JSON.stringify(before));

  await page.click('[data-toggle="wrap"]');
  const sourceWrapped = await page.evaluate(() => {
    const text = document.querySelector('.v2-src-text');
    return {
      hasWrapClass: !!document.querySelector('.v2-src.wrap'),
      buttonPressed: document.querySelector('[data-toggle="wrap"]')?.getAttribute('aria-pressed'),
      whiteSpace: getComputedStyle(text).whiteSpace,
      height: text.getBoundingClientRect().height,
    };
  });
  check('source wrap button applies wrapped source class',
    sourceWrapped.hasWrapClass === true && sourceWrapped.buttonPressed === 'true',
    JSON.stringify(sourceWrapped));
  check('source text uses wrapping whitespace',
    sourceWrapped.whiteSpace === 'pre-wrap' && sourceWrapped.height > before.height,
    JSON.stringify(sourceWrapped));

  await page.click('[data-mode="diff"]');
  await page.waitForSelector('.v2-diff', { timeout: 5000 });
  const diffWrapped = await page.evaluate(() => {
    const content = [...document.querySelectorAll('.v2-dl-content')].at(-1);
    return {
      hasWrapClass: !!document.querySelector('.v2-diff.wrap'),
      whiteSpace: getComputedStyle(content).whiteSpace,
      height: content.getBoundingClientRect().height,
    };
  });
  check('diff view inherits active wrap state',
    diffWrapped.hasWrapClass === true && diffWrapped.whiteSpace === 'pre-wrap' && diffWrapped.height > before.height,
    JSON.stringify(diffWrapped));

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
