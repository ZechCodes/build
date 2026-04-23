// Verify the new-channel modal:
//   - clicking a device's "+ New channel" button opens a modal
//     (not the old inline form)
//   - modal has Name, Working directory, plus Model + Effort
//     selects sourced from the device's harness list
//   - Create fires intent.create_channel with all the selected
//     values; Cancel + Esc dismiss without firing
//
// Non-destructive: stubs the E2EE conn.createChannel method.
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
  await page.waitForSelector('.v2-app', { timeout: 8000 });

  // Seed a synthetic device with a harness, stub the conn so Create
  // doesn't actually create a channel on the relay.
  const deviceId = 'test-dev-' + Date.now();
  await page.evaluate((deviceId) => {
    window.__v2debug.stores.devicesStore.upsert({ id: deviceId, name: 'Test Device', status: 'online', has_transport_key: true });
    window.__v2debug.stores.presenceStore.setHarnesses(deviceId, [
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
    const pool = window.__v2debug.e2eePool;
    pool._byDevice = pool._byDevice || new Map();
    pool._byDevice.set(deviceId, {
      connected: true,
      createChannel: async () => {},
    });
  }, deviceId);
  await page.waitForTimeout(150);

  // Wait for the seeded device to render.
  await page.waitForSelector(`.v2-device-group[data-device-id="${deviceId}"]`, { timeout: 3000 });

  // 1. The device row has a "+ New channel" button.
  const newBtn = page.locator(`.v2-device-group[data-device-id="${deviceId}"] [data-new-session]`);
  check('device row has a "+ New channel" button', (await newBtn.count()) === 1);

  // 2. Clicking it opens the modal (not the inline form).
  await newBtn.click({ force: true });
  await page.waitForSelector('.v2-modal', { timeout: 1500 });
  const fields = await page.evaluate(() => ({
    name:    !!document.querySelector('.v2-modal [data-new-field="name"]'),
    cwd:     !!document.querySelector('.v2-modal [data-new-field="cwd"]'),
    model:   !!document.querySelector('.v2-modal [data-new-field="model"]'),
    effort:  !!document.querySelector('.v2-modal [data-new-field="effort"]'),
    modelOpts:  [...document.querySelectorAll('.v2-modal [data-new-field="model"] option')].map(o => o.value),
    effortOpts: [...document.querySelectorAll('.v2-modal [data-new-field="effort"] option')].map(o => o.value),
  }));
  check('modal has a Name field',               fields.name);
  check('modal has a Working directory field',  fields.cwd);
  check('modal has a Model select',             fields.model);
  check('modal has an Effort select',           fields.effort);
  check('Model options come from the device\'s harness',
    JSON.stringify(fields.modelOpts) === JSON.stringify(['claude-sonnet-4-6', 'claude-opus-4-7']),
    JSON.stringify(fields.modelOpts));
  check('Effort options come from the harness',
    JSON.stringify(fields.effortOpts) === JSON.stringify(['low', 'medium', 'high']),
    JSON.stringify(fields.effortOpts));

  // 3. Fill + Create fires intent.create_channel with all fields.
  await page.evaluate(() => {
    window.__createCall = null;
    window.__v2debug.bus.on('intent.create_channel', (p) => { window.__createCall = p; });
  });
  await page.locator('.v2-modal [data-new-field="name"]').fill('My New Channel');
  await page.locator('.v2-modal [data-new-field="cwd"]').fill('/tmp/project');
  await page.locator('.v2-modal [data-new-field="model"]').selectOption('claude-opus-4-7');
  await page.locator('.v2-modal [data-new-field="effort"]').selectOption('high');
  await page.click('.v2-modal [data-modal-action="create"]');
  await page.waitForTimeout(120);
  const created = await page.evaluate(() => window.__createCall);
  check('Create fires intent.create_channel',
    created?.deviceId === deviceId
    && created?.name === 'My New Channel'
    && created?.working_directory === '/tmp/project'
    && created?.model === 'claude-opus-4-7'
    && created?.effort === 'high',
    JSON.stringify(created));

  const modalGone = !(await page.$('.v2-modal'));
  check('Modal dismisses after Create', modalGone);

  // 4. Name is required — Create without name shows an error toast + doesn't fire.
  await newBtn.click({ force: true });
  await page.waitForSelector('.v2-modal', { timeout: 1500 });
  await page.evaluate(() => { window.__createCall = null; });
  await page.click('.v2-modal [data-modal-action="create"]');
  await page.waitForTimeout(120);
  const blocked = await page.evaluate(() => window.__createCall);
  check('Create with empty Name does NOT fire the intent', blocked === null);

  // 5. Esc dismisses the modal.
  await page.keyboard.press('Escape');
  await page.waitForTimeout(80);
  const escGone = !(await page.$('.v2-modal'));
  check('Esc dismisses the modal', escGone);

  // 6. No inline form is rendered anywhere.
  const hasInlineForm = (await page.$$('.v2-new-session-form')).length > 0;
  check('old inline session form is gone', !hasInlineForm);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
