// Verify the current-tool strip above the chat composer:
//   - hidden when no tool is in flight,
//   - shows tag + summary on agent.tool_use,
//   - replaces itself when a new tool starts,
//   - hides on matching tool_result,
//   - hides when the agent goes idle (presenceStore.setAgentActive(false)).
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
      tag:  el.querySelector('.v2-chat-tool-tag')?.textContent?.trim(),
      desc: el.querySelector('.v2-chat-tool-desc')?.textContent?.trim(),
      tagClass: el.querySelector('.v2-chat-tool-tag')?.className,
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
  await page.goto(`${BASE}/dashboard-v2/`, { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });
  const chId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  await page.click(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
  await page.waitForTimeout(400);
  if (!(await page.$eval('#v2-chat-overlay', el => el.classList.contains('open')))) {
    await page.click('#v2-rail-chat-toggle');
    await page.waitForTimeout(250);
  }

  // 1. No tool in flight → strip hidden.
  let strip = await readStrip(page);
  check('strip element exists',        !!strip, JSON.stringify(strip));
  check('strip hidden with no tool',   strip?.hidden === true);

  // 2. Fire a Read tool_use → strip visible with READ tag + filename desc.
  await page.evaluate((chId) => {
    window.__v2debug.bus.emit('agent.tool_use', {
      channelId: chId,
      toolUseId: 't1',
      name: 'Read',
      input: { file_path: '/tmp/demo.py' },
      at: Date.now(),
    });
  }, chId);
  await page.waitForTimeout(80);
  strip = await readStrip(page);
  check('strip visible on tool_use',       strip?.hidden === false, JSON.stringify(strip));
  check('strip tag shows Read',            strip?.tag === 'Read', JSON.stringify(strip));
  check('strip tag has tool-colour class', /\bread\b/.test(strip?.tagClass || ''), strip?.tagClass);
  check('strip desc shows filename',       strip?.desc === 'demo.py', strip?.desc);

  // 3. Fire a second tool_use (Bash). The strip should REPLACE its content,
  //    not stack.
  await page.evaluate((chId) => {
    window.__v2debug.bus.emit('agent.tool_use', {
      channelId: chId,
      toolUseId: 't2',
      name: 'Bash',
      input: { command: 'ls -la /tmp' },
      at: Date.now(),
    });
  }, chId);
  await page.waitForTimeout(80);
  strip = await readStrip(page);
  check('strip tag flips to Bash',     strip?.tag === 'Bash', JSON.stringify(strip));
  check('strip desc shows command',     strip?.desc?.startsWith('ls -la'), strip?.desc);
  const stripCount = await page.evaluate(() => document.querySelectorAll('[data-slot="tool"]').length);
  check('only one strip rendered',      stripCount === 1, `count=${stripCount}`);

  // 4. Matching tool_result for t2 → strip hides.
  await page.evaluate((chId) => {
    window.__v2debug.bus.emit('agent.tool_result', {
      channelId: chId,
      toolUseId: 't2',
      isError: false,
      content: 'total 0',
      at: Date.now(),
    });
  }, chId);
  await page.waitForTimeout(80);
  strip = await readStrip(page);
  check('strip hides on matching result', strip?.hidden === true, JSON.stringify(strip));

  // 5. Fire another tool_use, then flip agentActive=false → strip hides
  //    even without a tool_result.
  await page.evaluate((chId) => {
    window.__v2debug.bus.emit('agent.tool_use', {
      channelId: chId,
      toolUseId: 't3',
      name: 'Grep',
      input: { pattern: 'TODO', path: '/tmp' },
      at: Date.now(),
    });
  }, chId);
  await page.waitForTimeout(80);
  strip = await readStrip(page);
  check('strip visible for t3 pre-idle', strip?.hidden === false);
  await page.evaluate((chId) => {
    // Ensure it's currently true so the setter fires a real transition.
    window.__v2debug.stores.presenceStore.setAgentActive(chId, true);
    window.__v2debug.stores.presenceStore.setAgentActive(chId, false);
  }, chId);
  await page.waitForTimeout(80);
  strip = await readStrip(page);
  check('strip hides when agent goes idle', strip?.hidden === true, JSON.stringify(strip));

  // 6. Cross-channel isolation: a tool_use for a different channel must
  //    NOT show in this channel's strip.
  await page.evaluate((chId) => {
    window.__v2debug.bus.emit('agent.tool_use', {
      channelId: `${chId}-other`,
      toolUseId: 'other-1',
      name: 'Read',
      input: { file_path: '/etc/hosts' },
      at: Date.now(),
    });
  }, chId);
  await page.waitForTimeout(80);
  strip = await readStrip(page);
  check('strip ignores other-channel tool', strip?.hidden === true, JSON.stringify(strip));

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
