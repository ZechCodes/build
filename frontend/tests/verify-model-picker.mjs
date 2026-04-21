// Verify the model/effort picker:
//   - clicking the ctrl pill opens a .v2-model-picker popover,
//   - it lists harness models + effort levels,
//   - clicking a model fires `intent.update_channel` with the new model,
//   - clicking outside closes the picker.
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
  await page.waitForTimeout(400);
  if (!(await page.$eval('#v2-chat-overlay', el => el.classList.contains('open')))) {
    await page.click('#v2-rail-chat-toggle');
    await page.waitForTimeout(250);
  }

  // Seed harnesses for the device so the picker has real options.
  await page.evaluate((chId) => {
    const d = window.__v2debug;
    const devId = d.stores.channelsStore.deviceFor(chId);
    d.stores.presenceStore.setHarnesses(devId, [
      {
        id: 'claude-code',
        name: 'Claude Code',
        models: [
          { id: 'claude-sonnet-4-6', name: 'Sonnet 4.6', provider: 'anthropic' },
          { id: 'claude-opus-4-7',   name: 'Opus 4.7',   provider: 'anthropic' },
        ],
        effort_levels: ['low', 'medium', 'high'],
      },
    ]);
    // Ensure the channel references this harness.
    d.stores.channelsStore.patch(chId, { harness: 'claude-code', model: 'claude-sonnet-4-6', effort: 'medium' });
  }, chId);
  await page.waitForTimeout(150);

  // Open the picker.
  await page.locator('[data-cmd="change-model"]').click();
  await page.waitForTimeout(150);

  const opened = await page.evaluate(() => {
    const p = document.querySelector('.v2-model-picker');
    return {
      present: !!p,
      modelCount: p?.querySelectorAll('[data-picker-model]').length || 0,
      effortCount: p?.querySelectorAll('[data-picker-effort]').length || 0,
      activeModel: p?.querySelector('[data-picker-model].active')?.getAttribute('data-picker-model'),
    };
  });
  check('picker opens on chip click',          opened.present, JSON.stringify(opened));
  check('picker lists 2 models',                opened.modelCount === 2);
  check('picker lists 3 effort levels',         opened.effortCount === 3);
  check('current model is marked active',       opened.activeModel === 'claude-sonnet-4-6');

  // Spy on intent.update_channel and click the other model.
  await page.evaluate(() => {
    window.__lastUpdateChannel = null;
    window.__v2debug.bus.on('intent.update_channel', (p) => { window.__lastUpdateChannel = p; });
  });
  await page.locator('[data-picker-model="claude-opus-4-7"]').click();
  await page.waitForTimeout(150);

  const after = await page.evaluate(() => ({
    picker: !!document.querySelector('.v2-model-picker'),
    lastUpdate: window.__lastUpdateChannel,
  }));
  check('selecting a model closes the picker', after.picker === false);
  check('selecting a model fires intent.update_channel',
    after.lastUpdate?.patch?.model === 'claude-opus-4-7',
    JSON.stringify(after.lastUpdate));

  // Re-open + click outside to close.
  await page.locator('[data-cmd="change-model"]').click();
  await page.waitForTimeout(100);
  await page.evaluate(() => {
    window.__v2debug.stores.messagesStore;  // touch store; pretend click elsewhere
  });
  // Click a completely unrelated body area.
  await page.mouse.click(5, 5);
  await page.waitForTimeout(100);
  const afterOutside = await page.evaluate(() => !!document.querySelector('.v2-model-picker'));
  check('clicking outside closes the picker', afterOutside === false);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
