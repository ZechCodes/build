import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8092';
const EMAIL = 'smoke@test.local';

const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await context.newPage();

const consoleErrors = [];
const pageErrors = [];
const networkErrors = [];
page.on('console', (msg) => {
  if (msg.type() === 'error') consoleErrors.push(msg.text());
});
page.on('pageerror', (err) => {
  pageErrors.push(`${err.name}: ${err.message}`);
});
page.on('response', (resp) => {
  if (resp.status() >= 500) networkErrors.push(`${resp.status()} ${resp.url()}`);
});

let passed = 0, failed = 0;
function check(label, ok, note) {
  if (ok) { passed++; console.log(`[PASS] ${label}${note ? ` — ${note}` : ''}`); }
  else { failed++; console.log(`[FAIL] ${label}${note ? ` — ${note}` : ''}`); }
}

try {
  // ============================================================
  // Login + dashboard load
  // ============================================================
  await page.goto(`${BASE}/auth/dummy/login`, { waitUntil: 'domcontentloaded' });
  await page.fill('input[name="email"]', EMAIL);
  const nameInput = await page.$('input[name="name"]');
  if (nameInput) await page.fill('input[name="name"]', 'Smoke User');
  await Promise.all([
    page.waitForURL((u) => !u.pathname.startsWith('/auth/'), { timeout: 10000 }).catch(() => null),
    page.click('button[type="submit"]'),
  ]);
  check('login redirects off auth', !page.url().includes('/auth/'), page.url());

  await page.goto(`${BASE}/dashboard`, { waitUntil: 'domcontentloaded' });
  await page.waitForTimeout(2000);

  // ============================================================
  // Bundle sanity
  // ============================================================
  const assets = await page.evaluate(() => ({
    links: [...document.querySelectorAll('link[rel="stylesheet"]')].map(l => l.href),
    scripts: [...document.querySelectorAll('script[src]')].map(s => s.src),
    inlineStyles: document.querySelectorAll('style').length,
  }));
  check('dashboard.css link present',
    assets.links.some(h => h.includes('/static/build/dist/css/dashboard.css')));
  check('dashboard.js script present',
    assets.scripts.some(s => s.includes('/static/build/dist/js/dashboard.js')));
  check('no inline <style> left', assets.inlineStyles === 0);

  // ============================================================
  // Core DOM
  // ============================================================
  for (const sel of [
    '.app.content-area',
    '#channel-panel-list',
    '#console-bottom',
    '#console-toggle',
    '#ui-toast',
    '#file-content-body',
    '#file-tree',
    '#terminal-output',
    '#chat-overlay',
    '#complications',
  ]) {
    check(`element ${sel} exists`, (await page.$(sel)) !== null);
  }

  // ============================================================
  // window.* bridges
  // ============================================================
  const globals = await page.evaluate(() => ({
    BuildE2EE: typeof window.BuildE2EE,
    renderMarkdown: typeof window.renderMarkdown,
    wordDiffLine: typeof window.wordDiffLine,
    highlightLine: typeof window.highlightLine,
    sodium: typeof window.sodium,
    switchTab: typeof window.switchTab,
    renderChannelPanel: typeof window.renderChannelPanel,
    getE2EE: typeof window.getE2EE,
    getActiveE2EE: typeof window.getActiveE2EE,
    anyE2EEConnected: typeof window.anyE2EEConnected,
    showBrowserView: typeof window.showBrowserView,
    connectToDevice: typeof window.connectToDevice,
    onTabSwitched: typeof window.onTabSwitched,
    __test_state: typeof window.__test_state,
    __test_bindE2EEEvents: typeof window.__test_bindE2EEEvents,
  }));
  for (const [name, type] of Object.entries(globals)) {
    check(`window.${name} exposed`, type === 'function' || (name === 'sodium' && type === 'object') || (name === '__test_state' && type === 'object'), type);
  }

  // ============================================================
  // Style + markdown smoke
  // ============================================================
  const bgColor = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
  check('body has computed background from bundled CSS', bgColor && bgColor !== 'rgba(0, 0, 0, 0)', bgColor);

  const mdHtml = await page.evaluate(() => window.renderMarkdown('# Hello\n\nThis is **bold** and `code`.'));
  check('renderMarkdown produces expected HTML',
    /<h1[\s>]/.test(mdHtml) && mdHtml.includes('<strong>bold</strong>') && mdHtml.includes('md-inline-code'));

  // ============================================================
  // Imperative UI calls
  // ============================================================
  for (const tab of ['files', 'chat', 'planning', 'console', 'files']) {
    const priorErrors = pageErrors.length;
    await page.evaluate((t) => window.switchTab(t), tab).catch((e) => pageErrors.push(`switchTab(${tab}): ${e.message}`));
    await page.waitForTimeout(60);
    check(`switchTab('${tab}') threw nothing`, pageErrors.length === priorErrors,
      pageErrors.slice(priorErrors).join('; '));
  }

  for (const [label, fn] of [
    ['toggleChatOverlay', 'window.toggleChatOverlay?.()'],
    ['setConsoleState open', "window.setConsoleState?.('open')"],
    ['setConsoleState collapsed', "window.setConsoleState?.('collapsed')"],
    ['addPendingFiles empty', 'window.addPendingFiles?.([])'],
  ]) {
    const priorErrors = pageErrors.length;
    await page.evaluate((code) => eval(code), fn).catch((e) => pageErrors.push(`${label}: ${e.message}`));
    await page.waitForTimeout(60);
    check(`${label} threw nothing`, pageErrors.length === priorErrors,
      pageErrors.slice(priorErrors).join('; '));
  }

  // ============================================================
  // Synthetic E2EE: register a fake instance + fire every event
  // ============================================================
  await page.evaluate(() => {
    class FakeE2EE extends EventTarget {
      constructor() {
        super();
        this.connected = true;
        this.sessionId = 'fake-session';
      }
      // No-op stubs for every public BuildE2EE method that bindE2EEEvents
      // (or downstream code) might invoke during event handling.
      async send() {} async sendMessage() {} async listChannels() {} async listHarnesses() {}
      async getMessages() {} async getActivity() {} async getComplications() {}
      async startAgent() {} async stopAgent() {} async restartAgent() {} async cancel() {}
      async createChannel() {} async renameChannel() {} async updateChannel() {} async deleteChannel() {}
      async resetSession() {} async compactSession() {} async markRead() {} async markSeen() {}
      async listWorkers() {} async filesList() {} async filesChanges() {}
      async fileRead() {} async fileDiff() {} async urlFetch() {}
      async terminalExec() {} async terminalKill() {} async terminalComplete() {}
      async uploadFile() { return { file_id: 'f', filename: 'x', size: 0, mime_type: '', path: '' }; }
      async sendInteractionResponse() {}
      disconnect() { this.connected = false; this.dispatchEvent(new Event('disconnected')); }
    }
    const fake = new FakeE2EE();
    window.__test_fake = fake;
    const s = window.__test_state;
    s.devices.set('dev-1', { id: 'dev-1', name: 'Fake', status: 'online' });
    s.e2eeConnections.set('dev-1', fake);
    s.deviceChannels.set('dev-1', new Map());
    window.__test_bindE2EEEvents(fake, 'dev-1');
  });

  // Each event row: { name, before?, detail, after?, label? }
  const events = [
    {
      name: 'connected',
      detail: undefined,
      after: "document.getElementById('chat-input')?.disabled === false",
    },
    {
      name: 'channel_list',
      detail: {
        channels: [{ id: 'ch-1', name: 'test', harness: 'claude', created_at: Date.now(), plan_mode: false }],
        agent_cwd: '/tmp',
      },
      after: "window.__test_state.chatChannels.has('ch-1')",
    },
    {
      name: 'harness_list',
      detail: [{ id: 'claude', name: 'Claude', models: [{ id: 'sonnet', name: 'Sonnet' }], effort_levels: ['low', 'high'], default_model: 'sonnet', default_effort: 'low' }],
      after: "window.__test_state.deviceHarnesses.get('dev-1').length === 1",
    },
    {
      name: 'channel_created',
      detail: { id: 'ch-2', name: 'new', harness: 'claude', created_at: Date.now() },
      after: "window.__test_state.chatChannels.has('ch-2')",
    },
    {
      name: 'channel_renamed',
      detail: { channel_id: 'ch-1', name: 'renamed' },
    },
    {
      name: 'channel_updated',
      detail: { channel_id: 'ch-1', model: 'sonnet', effort: 'high' },
    },
    {
      name: 'messages',
      detail: { channel_id: 'ch-1', messages: [{ id: 'm1', sender: 'user', content: 'hi', created_at: new Date().toISOString() }] },
      after: "(window.__test_state.chatMessages.get('ch-1')?.length || 0) >= 1",
    },
    {
      name: 'message',
      detail: { id: 'm2', channel_id: 'ch-1', sender: 'agent', content: 'hello', created_at: new Date().toISOString() },
    },
    {
      name: 'delivered',
      detail: { message_id: 'm1', channel_id: 'ch-1' },
    },
    {
      name: 'read',
      detail: { message_ids: ['m1'], channel_id: 'ch-1' },
    },
    {
      name: 'delivery_failed',
      detail: { message_id: 'm1', channel_id: 'ch-1' },
    },
    {
      // The legacy handler only logs — channelAgentActive is set by agent_event:chat.response.
      name: 'agent_started',
      detail: { channel_id: 'ch-1' },
    },
    {
      name: 'agent_event chat.response',
      eventName: 'agent_event',
      detail: { channel_id: 'ch-1', event_type: 'chat.response', event: { id: 'ae-1', sender: 'agent', content: 'response' } },
    },
    {
      name: 'agent_event activity.delta',
      eventName: 'agent_event',
      detail: { channel_id: 'ch-1', event_type: 'activity.delta', event: { kind: 'tool_use', tool_id: 't1', name: 'Read', input: { file_path: '/a' } } },
    },
    {
      name: 'agent_event activity.done',
      eventName: 'agent_event',
      detail: { channel_id: 'ch-1', event_type: 'activity.done', event: { tool_id: 't1', is_error: false, content: 'ok' } },
    },
    {
      name: 'activity_history',
      detail: { channel_id: 'ch-1', entries: [], total_tool_uses: 0 },
    },
    {
      name: 'agent_stopped',
      detail: { channel_id: 'ch-1' },
      after: "window.__test_state.channelAgentActive.get('ch-1') === false",
    },
    {
      name: 'agent_restarted',
      detail: { channel_id: 'ch-1' },
    },
    {
      name: 'worker_list',
      detail: [],
    },
    {
      name: 'chunk_ack',
      detail: { file_id: 'f1', chunk_index: 0 },
    },
    {
      name: 'upload_accepted',
      detail: { file_id: 'f1', filename: 'x.txt', size: 1, path: '/x.txt' },
    },
    {
      name: 'upload_error',
      detail: { file_id: 'f1', error: 'oops' },
    },
    {
      name: 'complication_update',
      detail: { id: 'c1', channel_id: 'ch-1', kind: 'git-status', data: { branch: 'main', insertions: 0, deletions: 0 }, timestamp: Date.now() },
      after: "window.__test_state.complicationState.get('ch-1')?.has('c1') === true",
    },
    {
      name: 'complication_remove',
      detail: { channel_id: 'ch-1', id: 'c1' },
      after: "window.__test_state.complicationState.get('ch-1')?.has('c1') !== true",
    },
    {
      name: 'complications',
      detail: { channel_id: 'ch-1', complications: [] },
    },
    {
      name: 'system_message',
      detail: { channel_id: 'ch-1', text: 'Session started' },
    },
    {
      name: 'plan_mode_updated',
      detail: { channel_id: 'ch-1', plan_mode: true },
      after: "window.__test_state.channelPlanMode.get('ch-1') === true",
    },
    {
      name: 'session_reset',
      detail: { channel_id: 'ch-1' },
    },
    {
      name: 'compact_started',
      detail: { channel_id: 'ch-1' },
    },
    {
      name: 'terminal_output streaming',
      eventName: 'terminal_output',
      detail: { channel_id: 'ch-1', data: 'line\n', done: false },
    },
    {
      name: 'terminal_output done',
      eventName: 'terminal_output',
      detail: { channel_id: 'ch-1', data: '', done: true, exit_code: 0, cwd: '/tmp' },
    },
    {
      name: 'terminal_completions',
      detail: { completions: ['foo', 'bar'] },
    },
    {
      name: 'files_list_result',
      before: "window.__test_state.filesChannelId = 'ch-1'",
      detail: { channel_id: 'ch-1', path: '', entries: [{ name: 'a.js', is_dir: false, size: 14 }] },
    },
    {
      name: 'files_changes_result',
      detail: { channel_id: 'ch-1', repos: [] },
    },
    {
      name: 'file_read_result text',
      eventName: 'file_read_result',
      before: "window.__test_state.filesChannelId = 'ch-1'; window.__test_state.filesCurrentPath = '/a.js'; window.__test_state.filesCurrentView = 'source'",
      detail: { channel_id: 'ch-1', path: '/a.js', content: 'console.log(1);', size: 14, truncated: false },
    },
    {
      // Exercises the fileContentBody.innerHTML branch — caught the
      // earlier regression where fileContentBody was undefined here.
      name: 'file_read_result error',
      eventName: 'file_read_result',
      before: "window.__test_state.filesChannelId = 'ch-1'; window.__test_state.filesCurrentPath = '/a.js'; window.__test_state.filesCurrentView = 'source'",
      detail: { channel_id: 'ch-1', path: '/a.js', error: 'permission denied' },
    },
    {
      name: 'file_read_result binary',
      eventName: 'file_read_result',
      before: "window.__test_state.filesChannelId = 'ch-1'; window.__test_state.filesCurrentPath = '/a.js'; window.__test_state.filesCurrentView = 'source'",
      detail: { channel_id: 'ch-1', path: '/a.js', is_binary: true, size: 1024 },
    },
    {
      name: 'file_read_result image',
      eventName: 'file_read_result',
      before: "window.__test_state.filesChannelId = 'ch-1'; window.__test_state.filesCurrentPath = '/img.png'; window.__test_state.filesCurrentView = 'source'",
      detail: { channel_id: 'ch-1', path: '/img.png', is_image: true, content: 'data:image/png;base64,iVBOR...' },
    },
    {
      name: 'file_diff_result',
      before: "window.__test_state.filesChannelId = 'ch-1'; window.__test_state.filesCurrentPath = '/a.js'; window.__test_state.filesCurrentView = 'diff'",
      detail: { channel_id: 'ch-1', path: '/a.js', diff: '@@ -1 +1 @@\n-old\n+new', truncated: false },
    },
    {
      name: 'url_fetch_result',
      detail: { request_id: 'r1', tab_id: '', status: 200, body: '<html></html>' },
    },
    {
      name: 'e2ee_error',
      detail: 'fake error',
    },
  ];

  for (const row of events) {
    const evtName = row.eventName || row.name;
    const priorErrors = pageErrors.length;
    try {
      await page.evaluate(({ evtName, before, detail }) => {
        if (before) eval(before);
        window.__test_fake.dispatchEvent(new CustomEvent(evtName, { detail }));
      }, { evtName, before: row.before, detail: row.detail });
      await page.waitForTimeout(40);
    } catch (e) {
      pageErrors.push(`dispatch(${row.name}): ${e.message}`);
    }
    check(`E2EE ${row.name}: no pageerror`, pageErrors.length === priorErrors,
      pageErrors.slice(priorErrors).join('; '));
    if (row.after) {
      let ok = false;
      try {
        ok = await page.evaluate((code) => !!eval(code), row.after);
      } catch (e) {
        ok = false;
      }
      check(`E2EE ${row.name}: post-condition`, ok, row.after);
    }
  }

  // Final disconnect — ensure the disconnected handler runs cleanly.
  {
    const priorErrors = pageErrors.length;
    await page.evaluate(() => window.__test_fake.dispatchEvent(new Event('disconnected')));
    await page.waitForTimeout(40);
    check('E2EE disconnected: no pageerror', pageErrors.length === priorErrors,
      pageErrors.slice(priorErrors).join('; '));
  }

  // ============================================================
  // Imperative state-mutating calls extracted into modules
  // ============================================================
  // Re-establish the fake connection (disconnected wiped it above).
  await page.evaluate(() => {
    window.__test_fake.connected = true;
    window.__test_fake.dispatchEvent(new CustomEvent('channel_list', {
      detail: { channels: [{ id: 'ch-1', name: 'test', harness: 'claude', created_at: Date.now() }], agent_cwd: '/tmp' },
    }));
  });
  await page.waitForTimeout(60);

  for (const [label, code, after] of [
    ['updatePlanModeUI(true)', "window.updatePlanModeUI?.(true)"],
    ['updatePlanModeUI(false)', "window.updatePlanModeUI?.(false)"],
    ['updateStopButton', "window.updateStopButton?.()"],
  ]) {
    const priorErrors = pageErrors.length;
    await page.evaluate((c) => eval(c), code).catch((e) => pageErrors.push(`${label}: ${e.message}`));
    await page.waitForTimeout(40);
    check(`${label} threw nothing`, pageErrors.length === priorErrors,
      pageErrors.slice(priorErrors).join('; '));
  }

  await page.screenshot({ path: 'dashboard.png', fullPage: false });

} catch (err) {
  console.error('[FATAL]', err.message, err.stack);
  failed++;
}

console.log('\n--- Console errors ---');
for (const e of consoleErrors) console.log('  console.error:', e);
console.log('\n--- Page errors ---');
for (const e of pageErrors) console.log('  pageerror:', e);
console.log('\n--- Network 5xx ---');
for (const e of networkErrors) console.log('  5xx:', e);

check('no uncaught JS errors', pageErrors.length === 0);
check('no 5xx responses', networkErrors.length === 0);

await browser.close();

console.log(`\n${passed} passed, ${failed} failed.`);
process.exit(failed === 0 ? 0 : 1);
