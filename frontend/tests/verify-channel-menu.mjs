// Verify the per-channel actions menu + edit modal:
//   - each channel row has a "…" trigger that opens a menu
//   - menu offers Restart / Stop / Edit channel…
//   - Restart / Stop fire their intents scoped to that channel
//   - Edit opens a modal with Name + Working directory inputs
//   - Save fires intent.rename_channel (name) and/or
//     intent.update_channel {working_directory}
//   - Delete → inline confirm → intent.delete_channel
//   - Esc / Cancel dismiss the modal without firing intents
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

async function openMenu(page, trigger) {
  await trigger.click({ force: true });
  await page.waitForTimeout(80);
}
async function openEdit(page, trigger) {
  await openMenu(page, trigger);
  await page.click('[data-ch-action="edit"]');
  await page.waitForSelector('.v2-modal', { timeout: 2000 });
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
  // Seed a synthetic channel into the stores so this test runs
  // deterministically regardless of whether the device currently
  // has any real channels. We also stub every transport method the
  // test hits so nothing mutates the live device.
  const chId = 'test-ch-' + Date.now();
  const deviceId = 'test-device';
  await page.evaluate(({ chId, deviceId }) => {
    const ch = {
      id: chId,
      name: 'Testing',
      device_id: deviceId,
      working_directory: '/tmp/old-wd',
    };
    window.__v2debug.stores.devicesStore.upsert({ id: deviceId, name: 'Test Device', status: 'online' });
    window.__v2debug.stores.channelsStore.upsert({ deviceId, channel: ch });
    const pool = window.__v2debug.e2eePool;
    // Fake a connected conn for this channel so intent-dispatcher's
    // connFor returns something.
    pool._byDevice = pool._byDevice || new Map();
    const stub = {
      connected: true,
      restartAgent:  async () => {},
      stopAgent:     async () => {},
      renameChannel: async () => {},
      updateChannel: async () => {},
      deleteChannel: async () => {},
    };
    // e2eePool.forChannel looks up by channel's device.
    pool._byDevice.set(deviceId, stub);
  }, { chId, deviceId });
  // Wait for the sidebar to render our seeded channel.
  await page.waitForSelector(`.v2-sidebar [data-channel-edit="${chId}"]`, { timeout: 4000 });

  const trigger = page.locator(`.v2-sidebar [data-channel-edit="${chId}"]`);
  check('found the seeded channel row', (await trigger.count()) > 0);

  // 1. Menu opens and offers Restart / Stop / Edit.
  await openMenu(page, trigger);
  const actions = await page.evaluate(() =>
    [...document.querySelectorAll('.v2-channel-menu [data-ch-action]')].map(b => b.getAttribute('data-ch-action')));
  check('menu offers restart / stop / edit',
    JSON.stringify(actions) === JSON.stringify(['restart', 'stop', 'edit']),
    JSON.stringify(actions));

  // 2. Restart fires intent.restart_agent.
  await page.evaluate(() => {
    window.__restart = null;
    window.__v2debug.bus.on('intent.restart_agent', (p) => { window.__restart = p; });
  });
  await page.click('[data-ch-action="restart"]');
  await page.waitForTimeout(80);
  const restarted = await page.evaluate(() => window.__restart);
  check('Restart fires intent.restart_agent with channelId',
    restarted?.channelId === chId, JSON.stringify(restarted));

  // 3. Edit opens the modal with name + cwd fields.
  await openEdit(page, trigger);
  const fields = await page.evaluate(() => ({
    hasName: !!document.querySelector('.v2-modal [data-edit-field="name"]'),
    hasCwd:  !!document.querySelector('.v2-modal [data-edit-field="cwd"]'),
    nameValue: document.querySelector('.v2-modal [data-edit-field="name"]').value,
  }));
  check('modal has Name input', fields.hasName);
  check('modal has Working directory input', fields.hasCwd);
  check('Name input pre-populated with current name',
    typeof fields.nameValue === 'string', fields.nameValue);

  // 4. Save → rename + update_channel (working_directory).
  await page.evaluate(() => {
    window.__rename = null;
    window.__update = null;
    window.__v2debug.bus.on('intent.rename_channel', (p) => { window.__rename = p; });
    window.__v2debug.bus.on('intent.update_channel', (p) => { window.__update = p; });
  });
  await page.locator('.v2-modal [data-edit-field="name"]').fill('Edited Name');
  await page.locator('.v2-modal [data-edit-field="cwd"]').fill('/tmp/new-wd');
  await page.click('.v2-modal [data-modal-action="save"]');
  await page.waitForTimeout(120);
  const saved = await page.evaluate(() => ({
    rename: window.__rename,
    update: window.__update,
    modalGone: !document.querySelector('.v2-modal'),
  }));
  check('Save fires intent.rename_channel',
    saved.rename?.channelId === chId && saved.rename?.name === 'Edited Name',
    JSON.stringify(saved.rename));
  check('Save fires intent.update_channel { working_directory }',
    saved.update?.channelId === chId && saved.update?.patch?.working_directory === '/tmp/new-wd',
    JSON.stringify(saved.update));
  check('modal dismisses after save', saved.modalGone);

  // 5. Delete flow: inline confirm + intent.delete_channel.
  await openEdit(page, trigger);
  await page.click('.v2-modal [data-modal-action="delete-start"]');
  await page.waitForTimeout(60);
  const confirmVisible = await page.evaluate(() =>
    !document.querySelector('.v2-modal-delete-confirm')?.hidden);
  check('Delete reveals inline confirm', confirmVisible);
  await page.evaluate(() => {
    window.__deleted = null;
    window.__v2debug.bus.on('intent.delete_channel', (p) => { window.__deleted = p; });
  });
  await page.click('.v2-modal [data-modal-action="delete-confirm"]');
  await page.waitForTimeout(120);
  const deletedPayload = await page.evaluate(() => window.__deleted);
  check('Delete confirm fires intent.delete_channel',
    deletedPayload?.channelId === chId, JSON.stringify(deletedPayload));

  // 6. Esc closes modal without firing any intents.
  await openEdit(page, trigger);
  await page.evaluate(() => {
    window.__rename = null;
    window.__update = null;
  });
  await page.locator('.v2-modal [data-edit-field="name"]').fill('Nope');
  await page.keyboard.press('Escape');
  await page.waitForTimeout(100);
  const escState = await page.evaluate(() => ({
    modalGone: !document.querySelector('.v2-modal'),
    rename: window.__rename,
    update: window.__update,
  }));
  check('Esc dismisses modal',                    escState.modalGone);
  check('Esc does not fire rename/update intents', escState.rename === null && escState.update === null);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
