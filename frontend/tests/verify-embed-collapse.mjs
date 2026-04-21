// Verify chat embeds start collapsed when > 10 lines and expand on
// header click. Covers:
//   1. build-file embed (from <build-file> syntax) collapses at 10+ lines.
//   2. fenced ``` code block collapses at 10+ lines.
//   3. Header click toggles .collapsed on/off.
//   4. Short embeds (≤10 lines) render fully expanded (no .collapsed).
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
  await page.waitForTimeout(500);
  if (!(await page.$eval('#v2-chat-overlay', el => el.classList.contains('open')))) {
    await page.click('#v2-rail-chat-toggle');
    await page.waitForTimeout(300);
  }

  // Inject two messages directly into messagesStore:
  //   - one big file embed (15 lines) → collapsed
  //   - one big fenced code block (12 lines) → collapsed
  //   - one short embed (5 lines) → expanded
  const longFile = Array.from({ length: 15 }, (_, i) => `line ${i + 1}`).join('\n');
  const shortFile = Array.from({ length: 5 }, (_, i) => `short ${i + 1}`).join('\n');
  const longCode = Array.from({ length: 12 }, (_, i) => `printf("%d\\n", ${i + 1});`).join('\n');

  const LONG_MSG = 'test-embed-long-msg';
  const SHORT_MSG = 'test-embed-short-msg';
  await page.evaluate(({ id, longFile, shortFile, longCode, longId, shortId }) => {
    const s = window.__v2debug.stores.messagesStore;
    const now = new Date().toISOString();
    s.append(id, {
      id: longId,
      channel_id: id, sender: 'Agent',
      content: `Long file:\n\n<build-file path="long.txt" lang="txt">\n${longFile}\n</build-file>\n\nAlso a long code block:\n\n\`\`\`c\n${longCode}\n\`\`\``,
      created_at: now,
    });
    s.append(id, {
      id: shortId,
      channel_id: id, sender: 'Agent',
      content: `Short embed:\n\n<build-file path="short.txt" lang="txt">\n${shortFile}\n</build-file>`,
      created_at: now,
    });
  }, { id: chId, longFile, shortFile, longCode, longId: LONG_MSG, shortId: SHORT_MSG });
  await page.waitForTimeout(200);

  const longSel  = `.v2-msg[data-msg-id="${LONG_MSG}"] .build-embed[data-embed-type="file"]`;
  const codeSel  = `.v2-msg[data-msg-id="${LONG_MSG}"] .md-code-block`;
  const shortSel = `.v2-msg[data-msg-id="${SHORT_MSG}"] .build-embed[data-embed-type="file"]`;

  // 1. Long file embed should be collapsed.
  const longFileState = await page.evaluate((sel) => {
    const el = document.querySelector(sel);
    return {
      collapsed: el?.classList.contains('collapsed'),
      moreBadge: el?.querySelector('.build-embed-more')?.textContent?.trim(),
      bodyH: Math.round(el?.querySelector('.build-embed-body')?.getBoundingClientRect().height ?? -1),
    };
  }, longSel);
  check('long file embed starts collapsed', longFileState.collapsed === true);
  check('collapsed file embed shows "+N lines" badge',
    /\+5\s*lines/i.test(longFileState.moreBadge || ''), longFileState.moreBadge);
  check('collapsed body height capped ≤ ~130px',
    longFileState.bodyH > 0 && longFileState.bodyH <= 135,
    String(longFileState.bodyH));

  // 2. Long code block should be collapsed.
  const longCodeState = await page.evaluate((sel) => {
    const el = document.querySelector(sel);
    return {
      collapsed: el?.classList.contains('collapsed'),
      moreBadge: el?.querySelector('.md-code-more')?.textContent?.trim(),
    };
  }, codeSel);
  check('long fenced code block starts collapsed', longCodeState.collapsed === true);
  check('collapsed code block shows "+N lines" badge',
    /\+2\s*lines/i.test(longCodeState.moreBadge || ''), longCodeState.moreBadge);

  // 3. Short embed renders expanded (no .collapsed).
  const shortState = await page.evaluate((sel) => {
    const el = document.querySelector(sel);
    return { collapsed: el?.classList.contains('collapsed') };
  }, shortSel);
  check('short file embed is NOT collapsed', shortState.collapsed === false);

  // 4. Clicking the header toggles collapsed.
  await page.locator(longSel).locator('.build-embed-header').click();
  await page.waitForTimeout(120);
  const afterExpand = await page.evaluate((sel) =>
    document.querySelector(sel)?.classList.contains('collapsed'), longSel);
  check('clicking header expands the embed', afterExpand === false);

  await page.locator(longSel).locator('.build-embed-header').click();
  await page.waitForTimeout(120);
  const afterCollapse = await page.evaluate((sel) =>
    document.querySelector(sel)?.classList.contains('collapsed'), longSel);
  check('clicking header again re-collapses', afterCollapse === true);

  // 5. Clicking the code-block header toggles its collapsed state.
  await page.locator(codeSel).locator('.md-code-header').click();
  await page.waitForTimeout(120);
  const codeAfter = await page.evaluate((sel) =>
    document.querySelector(sel)?.classList.contains('collapsed'), codeSel);
  check('clicking code-block header expands it', codeAfter === false);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
