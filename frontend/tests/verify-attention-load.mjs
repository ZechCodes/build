// Verify the Attention section populates on fresh connect/reconnect
// via the attention-hydrator:
//   1. A channel.list event triggers intent.get_messages per channel.
//   2. message.bulk hydrates the unreadStore with derived counts +
//      interaction flags.
//   3. The sidebar's Attention section renders the waiting row
//      without any live message.received / interaction.requested
//      events firing.
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
  await page.goto(`${BASE}/dashboard/`, { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });

  // Use the existing dev channel (testing).
  const chId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  const devId = await page.$eval('.v2-device-group', el => el.dataset.deviceId);

  // Clean slate: clear unread state + active channel so Attention
  // starts empty.
  await page.evaluate((chId) => {
    const d = window.__v2debug;
    d.stores.unreadStore.markRead(chId);
    d.stores.presenceStore.setAgentActive(chId, false);
    d.stores.uiStore.setActiveChannel(null);
  }, chId);
  await page.waitForTimeout(120);

  const startClean = await page.evaluate(() => !!document.querySelector('.v2-attention-section'));
  check('Attention section empty to start', startClean === false);

  // ---- 1. Verify channel.list fan-out: emit it and spy on
  //         intent.get_messages.
  const afterChannelList = await page.evaluate(({ devId, chId }) => {
    window.__getMsgsCalls = [];
    const off = window.__v2debug.bus.on('intent.get_messages', (p) => {
      window.__getMsgsCalls.push(p);
    });
    window.__v2debug.bus.emit('channel.list', {
      deviceId: devId,
      channels: [{ id: chId }],
    });
    // Don't leave the spy listener in place.
    off();
    return window.__getMsgsCalls;
  }, { devId, chId });
  check('channel.list triggers intent.get_messages', afterChannelList.length === 1,
    JSON.stringify(afterChannelList));
  check('intent.get_messages targets the right channel',
    afterChannelList[0]?.channelId === chId);
  check('intent.get_messages requests a bulk limit',
    afterChannelList[0]?.limit >= 100);

  // ---- 2. Emit a message.bulk with 2 unread + a pending plan.
  // Wait a beat to let any in-flight REAL message.bulk (from the
  // initial e2ee connect) settle, so our synthetic hydrate wins.
  await page.waitForTimeout(800);
  const INT_ID = 'load-int-' + Date.now();
  await page.evaluate(({ chId, intId }) => {
    // The hasInteraction flag considers only the LATEST non-client
    // message. For this test we want the pending plan_review to be
    // that message, so it's placed AFTER older Agent messages.
    const bulk = [
      { id: 'm-read', sender: 'Agent', content: 'old', read_at: '2026-01-01T00:00:00Z', created_at: '2026-01-01T00:00:00Z' },
      { id: 'm-a', sender: 'Agent', content: 'Hi', read_at: null, created_at: new Date().toISOString() },
      { id: 'm-b', sender: 'Agent', content: 'Still there?', read_at: null, created_at: new Date().toISOString() },
      {
        id: intId,
        sender: 'Agent',
        content: 'Please review',
        metadata: JSON.stringify({ interaction_id: intId, kind: 'plan_review', plan: 'x' }),
        read_at: null,
        created_at: new Date().toISOString(),
      },
      // Client "typing" AFTER the interaction should NOT supersede
      // it — the derive walks past client messages.
      { id: 'm-self', sender: 'client', content: 'hi back', read_at: null, created_at: new Date().toISOString() },
    ];
    window.__v2debug.bus.emit('message.bulk', { channelId: chId, msgs: bulk });
  }, { chId, intId: INT_ID });
  await page.waitForTimeout(200);

  const hydrated = await page.evaluate((id) => {
    const slot = window.__v2debug.stores.unreadStore.get(id);
    return { count: slot.count, hasInteraction: slot.hasInteraction };
  }, chId);
  check('unreadStore count hydrated from bulk',  hydrated.count === 3,              JSON.stringify(hydrated));
  check('unreadStore flags pending interaction', hydrated.hasInteraction === true);

  // ---- 3. Attention sidebar renders the row.
  const sidebar = await page.evaluate(() => {
    const row = document.querySelector('.v2-attention-row');
    return {
      present: !!document.querySelector('.v2-attention-section'),
      hasInteractionClass: row?.className.includes('has-interaction'),
      statusLabel: row?.querySelector('.v2-attention-status')?.textContent?.trim(),
      badge: row?.querySelector('.v2-ch-unread-badge')?.textContent?.trim(),
    };
  });
  check('Attention section appears after hydration',  sidebar.present);
  check('Attention row has .has-interaction class',   sidebar.hasInteractionClass === true);
  check('Status label reads "Needs you"',             /needs you/i.test(sidebar.statusLabel || ''), sidebar.statusLabel);
  check('Unread badge shows the hydrated count',      sidebar.badge === '3', sidebar.badge);

  // ---- 4. Unread via last_seen_at + recent via latestActivityMs.
  // Seed a fresh channel with `last_seen_at = T` and an agent
  // message at `T + 1h`. Expect the channel to flag unread + recent.
  const UNREAD_CH = 'load-unread-' + Date.now();
  await page.evaluate(({ deviceId, chId }) => {
    const d = window.__v2debug;
    const lastSeenAt = new Date(Date.now() - 2 * 60 * 60 * 1000).toISOString();  // 2h ago
    // Inject a channel with last_seen_at set.
    d.stores.channelsStore.upsert({
      deviceId,
      channel: {
        id: chId, name: 'Verify Unread',
        last_seen_at: lastSeenAt,
        created_at: lastSeenAt,
      },
    });
  }, { deviceId: 'verify-device', chId: UNREAD_CH });

  // Bulk that postdates last_seen_at — should count as unread even
  // though read_at is stamped.
  await page.evaluate(({ chId }) => {
    const recentAt = new Date(Date.now() - 30 * 60 * 1000).toISOString();  // 30m ago
    window.__v2debug.bus.emit('message.bulk', {
      channelId: chId,
      msgs: [
        { id: 'u-a', sender: 'Agent', content: 'while you were out', read_at: null, created_at: recentAt },
        { id: 'u-b', sender: 'Agent', content: 'more',                  read_at: '2026-01-01Z', created_at: recentAt },
      ],
    });
  }, { chId: UNREAD_CH });
  await page.waitForTimeout(200);

  const unreadState = await page.evaluate((chId) => {
    const slot = window.__v2debug.stores.unreadStore.get(chId);
    const pres = window.__v2debug.stores.presenceStore.get(chId);
    return {
      count: slot.count,
      lastActiveRecent: pres.lastActiveAt > 0 && (Date.now() - pres.lastActiveAt) < 60 * 60 * 1000,
    };
  }, UNREAD_CH);
  check('newer-than-last_seen message counted as unread',  unreadState.count === 2, JSON.stringify(unreadState));
  check('lastActiveAt hydrated from latest message',       unreadState.lastActiveRecent === true);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
