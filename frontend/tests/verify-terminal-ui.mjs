// Verify the terminal UX polish:
//   - ⌃C button is always visible; wires to intent.terminal_kill
//     while running and echoes ^C while idle
//   - mobile viewport exposes Tab + Esc buttons (hidden on desktop)
//   - ANSI output renders as spans with ansi-fg-N classes
//   - the "is-running" root class flips while a command is in flight
import { chromium, devices } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';

let passed = 0, failed = 0;
function check(label, ok, note) {
  if (ok) { passed++; console.log(`[PASS] ${label}${note ? ` — ${note}` : ''}`); }
  else    { failed++; console.log(`[FAIL] ${label}${note ? ` — ${note}` : ''}`); }
}

async function login(page) {
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
  // Activate the first channel via the store to avoid mobile-sidebar
  // viewport issues and the sidebar/attention duplicate-class trap.
  const chId = await page.evaluate(() => {
    const el = document.querySelector('.v2-sidebar .v2-channel-sidebar-item');
    const id = el?.getAttribute('data-channel-id');
    if (id) window.__v2debug.stores.uiStore.setActiveChannel(id);
    return id;
  });
  await page.waitForTimeout(400);
  // Open the terminal rail panel.
  await page.evaluate(() => window.__v2debug.stores.uiStore.setRailPanel('terminal'));
  await page.waitForSelector('.v2-term-input', { timeout: 5000 });
  return chId;
}

const browser = await chromium.launch({ headless: true });
try {
  // ── Desktop: controls visible + ANSI + ⌃C flows ─────────────────
  {
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    const page = await ctx.newPage();
    const chId = await login(page);

    // 1. ⌃C button present + always visible. Tab/Esc buttons exist but hidden
    //    on desktop.
    const btns = await page.evaluate(() => ({
      ctrlC:     document.querySelector('[data-term-cmd="ctrl-c"]'),
      tab:       document.querySelector('[data-term-cmd="tab"]'),
      esc:       document.querySelector('[data-term-cmd="esc"]'),
    }));
    check('Ctrl+C button rendered', btns.ctrlC != null);
    check('Tab button rendered in DOM', btns.tab != null);
    check('Esc button rendered in DOM', btns.esc != null);

    const vis = await page.evaluate(() => ({
      ctrlC: getComputedStyle(document.querySelector('[data-term-cmd="ctrl-c"]')).display,
      tab:   getComputedStyle(document.querySelector('[data-term-cmd="tab"]')).display,
      esc:   getComputedStyle(document.querySelector('[data-term-cmd="esc"]')).display,
    }));
    check('⌃C visible on desktop',    vis.ctrlC !== 'none', vis.ctrlC);
    check('Tab hidden on desktop',    vis.tab === 'none',  vis.tab);
    check('Esc hidden on desktop',    vis.esc === 'none',  vis.esc);

    // 2. Seed a coloured output through the terminal store; expect ANSI spans.
    await page.evaluate((id) => {
      const s = window.__v2debug.stores.terminalStore;
      s.appendOutput(id, '\x1b[31merror:\x1b[0m something broke\n\x1b[32mOK\x1b[0m\n');
    }, chId);
    await page.waitForTimeout(80);

    const ansi = await page.evaluate(() => {
      const out = document.querySelector('.v2-term-output .v2-term-out');
      return {
        hasRed:   !!out?.querySelector('.ansi-fg-1'),
        hasGreen: !!out?.querySelector('.ansi-fg-2'),
        text:     out?.textContent,
      };
    });
    check('ANSI red rendered as .ansi-fg-1', ansi.hasRed, JSON.stringify(ansi));
    check('ANSI green rendered as .ansi-fg-2', ansi.hasGreen);
    check('escape bytes stripped from rendered text',
      !/\x1b/.test(ansi.text || ''), `text=${JSON.stringify(ansi.text)}`);

    // 3. ⌃C while running → fires intent.terminal_kill.
    await page.evaluate((id) => {
      window.__lastKill = null;
      window.__v2debug.bus.on('intent.terminal_kill', (p) => { window.__lastKill = p; });
      // Simulate a command is running.
      const s = window.__v2debug.stores.terminalStore;
      s.appendOutput(id, 'running…');
    }, chId);
    await page.waitForTimeout(80);
    // Confirm the is-running root class flipped.
    const runningClass = await page.evaluate(() => document.querySelector('.v2-term')?.classList.contains('is-running'));
    check('is-running class present while running', runningClass === true);

    await page.click('[data-term-cmd="ctrl-c"]');
    await page.waitForTimeout(80);
    const killPayload = await page.evaluate(() => window.__lastKill);
    check('⌃C while running fires intent.terminal_kill',
      killPayload && killPayload.channelId === chId, JSON.stringify(killPayload));

    // 4. Mark complete, then ⌃C while idle → echo line with ^C.
    await page.evaluate((id) => {
      window.__v2debug.stores.terminalStore.markComplete(id, 130, '/tmp');
      document.querySelector('.v2-term-input').value = 'half typed';
    }, chId);
    await page.waitForTimeout(80);
    const idleClass = await page.evaluate(() => document.querySelector('.v2-term')?.classList.contains('is-running'));
    check('is-running class removed when idle', idleClass === false);

    await page.click('[data-term-cmd="ctrl-c"]');
    await page.waitForTimeout(80);
    const echoed = await page.evaluate(() => {
      const last = document.querySelector('.v2-term-output .v2-term-echo:last-child');
      return last ? {
        cmd: last.querySelector('.v2-term-echo-cmd')?.textContent,
        trail: last.querySelector('.v2-term-echo-trail')?.textContent,
        inputVal: document.querySelector('.v2-term-input').value,
      } : null;
    });
    check('⌃C while idle echoes typed line with ^C',
      echoed?.cmd === 'half typed' && echoed?.trail === '^C' && echoed?.inputVal === '',
      JSON.stringify(echoed));

    await ctx.close();
  }

  // ── Mobile: Tab/Esc buttons visible ─────────────────────────────
  {
    const ctx = await browser.newContext({ ...devices['iPhone 14'] });
    const page = await ctx.newPage();
    await login(page);
    const vis = await page.evaluate(() => ({
      ctrlC: getComputedStyle(document.querySelector('[data-term-cmd="ctrl-c"]')).display,
      tab:   getComputedStyle(document.querySelector('[data-term-cmd="tab"]')).display,
      esc:   getComputedStyle(document.querySelector('[data-term-cmd="esc"]')).display,
    }));
    check('⌃C visible on mobile',  vis.ctrlC !== 'none');
    check('Tab visible on mobile', vis.tab !== 'none', vis.tab);
    check('Esc visible on mobile', vis.esc !== 'none', vis.esc);

    // Tap Tab → fires intent.terminal_complete.
    const chId = await page.evaluate(() => window.__v2debug.stores.uiStore.getActiveChannel());
    await page.evaluate(() => {
      window.__lastComp = null;
      window.__v2debug.bus.on('intent.terminal_complete', (p) => { window.__lastComp = p; });
      document.querySelector('.v2-term-input').value = 'ls ~/Pro';
    });
    await page.tap('[data-term-cmd="tab"]');
    await page.waitForTimeout(80);
    const comp = await page.evaluate(() => window.__lastComp);
    check('Tap Tab dispatches intent.terminal_complete',
      comp && comp.channelId === chId, JSON.stringify(comp));

    await ctx.close();
  }

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
