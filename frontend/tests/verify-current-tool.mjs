// Verify the current-tool strip above the chat composer:
//   - hidden when no tool in flight AND agent is idle,
//   - shows "Thinking" when agent is active with no tool,
//   - shows phrase-style text on agent.tool_use ("Reading /foo.py",
//     "Running `ls`", "Searching /pattern/"),
//   - a newer tool_use replaces the previous immediately (no queue),
//   - when a tool finishes (tool_result) within 2s of appearing, the
//     flip to "Thinking" is deferred until the 2s minimum elapses,
//   - hides when presenceStore flips agent_active → false.
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

async function readStrip(page) {
  return page.evaluate(() => {
    const el = document.querySelector('[data-slot="tool"]');
    if (!el) return null;
    return {
      hidden: el.hidden,
      text: el.querySelector('.v2-chat-tool-text')?.textContent ?? '',
      hasEllipsis: !!el.querySelector('.v2-chat-tool-ellipsis'),
    };
  });
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
  if (!(await page.$eval('#v2-chat-overlay', el => el.classList.contains('open')))) {
    await page.click('#v2-rail-chat-toggle');
    await page.waitForTimeout(250);
  }

  // Force agent inactive so we start from a clean "idle" baseline.
  await page.evaluate((id) => {
    const d = window.__v2debug;
    d.stores.presenceStore.setAgentActive(id, true);
    d.stores.presenceStore.setAgentActive(id, false);
    // Also clear any latched currentToolStore entry.
    d.stores.currentToolStore?.clear?.(id);
  }, chId);
  await page.waitForTimeout(50);

  // 1. Idle → strip hidden.
  let strip = await readStrip(page);
  check('strip element exists',       !!strip, JSON.stringify(strip));
  check('strip hidden when idle',     strip?.hidden === true);
  check('strip has an ellipsis node', strip?.hasEllipsis === true);

  // 2. Agent active, no tool → "Thinking".
  await page.evaluate((id) => {
    window.__v2debug.stores.presenceStore.setAgentActive(id, true);
  }, chId);
  await page.waitForTimeout(50);
  strip = await readStrip(page);
  check('strip visible when agent active',      strip?.hidden === false, JSON.stringify(strip));
  check('strip shows "Thinking" with no tool',  strip?.text === 'Thinking', strip?.text);

  // 3. Read tool → "Reading /tmp/demo.py".
  await page.evaluate((id) => {
    window.__v2debug.bus.emit('agent.tool_use', {
      channelId: id,
      toolUseId: 't1',
      name: 'Read',
      input: { file_path: '/tmp/demo.py' },
    });
  }, chId);
  await page.waitForTimeout(50);
  strip = await readStrip(page);
  check('Read phrase',     strip?.text === 'Reading /tmp/demo.py', strip?.text);

  // 4. Immediately fire a second tool_use (Bash). New one REPLACES the
  //    first — no queue. This happens well within the 2s window, so the
  //    new-tool rule wins.
  await page.evaluate((id) => {
    window.__v2debug.bus.emit('agent.tool_use', {
      channelId: id,
      toolUseId: 't2',
      name: 'Bash',
      input: { command: 'ls -la /tmp' },
    });
  }, chId);
  await page.waitForTimeout(50);
  strip = await readStrip(page);
  check('Bash phrase replaces Read immediately',
    strip?.text === 'Running `ls -la /tmp`', strip?.text);

  // 5. Fire tool_result for t2 quickly (< 2s since t2 appeared). The
  //    flip to "Thinking" should be DEFERRED until the 2s minimum
  //    elapses. Check mid-window.
  await page.evaluate((id) => {
    window.__v2debug.bus.emit('agent.tool_result', {
      channelId: id,
      toolUseId: 't2',
      isError: false,
      content: 'total 0',
    });
  }, chId);
  await page.waitForTimeout(250);
  strip = await readStrip(page);
  check('Bash phrase lingers during 2s minimum',
    strip?.text === 'Running `ls -la /tmp`', strip?.text);

  // Now wait out the 2s (we've already waited ~0.35s combined). Give
  // another 2000ms to be safe against scheduler jitter.
  await page.waitForTimeout(2100);
  strip = await readStrip(page);
  check('flip to "Thinking" after 2s elapsed', strip?.text === 'Thinking', strip?.text);

  // 6. Grep phrasing.
  await page.evaluate((id) => {
    window.__v2debug.bus.emit('agent.tool_use', {
      channelId: id,
      toolUseId: 't3',
      name: 'Grep',
      input: { pattern: 'TODO', path: '/tmp' },
    });
  }, chId);
  await page.waitForTimeout(50);
  strip = await readStrip(page);
  check('Grep phrase uses /pattern/ style',
    strip?.text === 'Searching /TODO/', strip?.text);

  // 7. Agent goes idle → strip hides (even without a tool_result).
  await page.evaluate((id) => {
    window.__v2debug.stores.presenceStore.setAgentActive(id, false);
  }, chId);
  await page.waitForTimeout(80);
  strip = await readStrip(page);
  check('strip hides when agent goes idle', strip?.hidden === true, JSON.stringify(strip));

  // 8. Cross-channel isolation: a tool_use for a different channel does
  //    not leak into this channel's strip.
  await page.evaluate((id) => {
    window.__v2debug.bus.emit('agent.tool_use', {
      channelId: `${id}-other`,
      toolUseId: 'o1',
      name: 'Read',
      input: { file_path: '/etc/hosts' },
    });
  }, chId);
  await page.waitForTimeout(50);
  strip = await readStrip(page);
  check('strip ignores other-channel tool', strip?.hidden === true, JSON.stringify(strip));

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
