// Verify the new Attention section in the sidebar + the rail's
// scoped-to-active-channel unread badge.
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
  try {
    await page.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });
  } catch (_) {
    await page.evaluate(() => {
      const d = window.__v2debug;
      d.bus.emit('device.bulk', {
        devices: [{ id: 'dev-attention', name: 'Attention Device', status: 'online', has_transport_key: true }],
      });
      d.bus.emit('channel.list', {
        deviceId: 'dev-attention',
        channels: [{ id: 'ch-attention', name: 'attention', created_at: Date.now() }],
      });
    });
    await page.waitForSelector('.v2-channel-sidebar-item', { timeout: 5000 });
  }

  const chId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  await page.click(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
  await page.waitForTimeout(300);

  // No attention signals yet → the section should NOT render.
  await page.evaluate(() => {
    const d = window.__v2debug;
    const chId = d.stores.uiStore.getActiveChannel();
    // Null out any latched state from prior sessions so we start clean.
    d.stores.presenceStore.setAgentActive(chId, false);
    d.stores.unreadStore.markRead(chId);
  });
  await page.waitForTimeout(100);
  const clean = await page.evaluate(() => !!document.querySelector('.v2-attention-section'));
  check('Attention section is hidden when nothing needs attention', clean === false);

  // Running + unread: running remains the primary visual status, and
  // unread is represented by the count badge only.
  await page.evaluate(() => {
    const d = window.__v2debug;
    const chId = d.stores.uiStore.getActiveChannel();
    d.stores.presenceStore.setAgentActive(chId, true);
    d.stores.unreadStore.increment(chId, false);
  });
  await page.waitForTimeout(120);
  const running = await page.evaluate(() => {
    const row = document.querySelector('.v2-attention-row');
    return {
      present: !!document.querySelector('.v2-attention-section'),
      statusLabel: row?.querySelector('.v2-attention-status')?.textContent?.trim(),
      hasRunningClass: row?.className.includes('status-running'),
      hasUnreadClass: row?.className.includes('status-unread'),
      badge: row?.querySelector('.v2-ch-unread-badge')?.textContent?.trim(),
      indicator: row?.querySelector('.v2-attention-indicator')?.getAttribute('data-indicator'),
    };
  });
  check('Attention section appears when agent is running',  running.present);
  check('Running row has status=running',                    running.hasRunningClass === true, JSON.stringify(running));
  check('Running row status label reads "Running"',          /running/i.test(running.statusLabel || ''));
  check('Running row with unread keeps unread badge only',    running.badge === '1' && running.hasUnreadClass === false, JSON.stringify(running));

  // Stop running while unread remains: unread becomes the primary
  // status.
  await page.evaluate(() => {
    const d = window.__v2debug;
    const chId = d.stores.uiStore.getActiveChannel();
    d.stores.presenceStore.setAgentActive(chId, false);
  });
  await page.waitForTimeout(120);
  const stoppedUnread = await page.evaluate(() => {
    const row = document.querySelector('.v2-attention-row');
    return {
      hasUnreadClass: row?.className.includes('status-unread'),
      statusLabel: row?.querySelector('.v2-attention-status')?.textContent?.trim(),
      badge: row?.querySelector('.v2-ch-unread-badge')?.textContent?.trim(),
    };
  });
  check('Stopped unread row switches to status=unread',       stoppedUnread.hasUnreadClass === true, JSON.stringify(stoppedUnread));
  check('Stopped unread row label reads "Unread"',            /unread/i.test(stoppedUnread.statusLabel || ''), stoppedUnread.statusLabel);
  check('Stopped unread row keeps unread count badge',        stoppedUnread.badge === '1', stoppedUnread.badge);

  // Switch away + simulate pending interaction on another channel.
  await page.evaluate(() => {
    const d = window.__v2debug;
    const chId = d.stores.uiStore.getActiveChannel();
    d.stores.unreadStore.markRead(chId);
    d.stores.uiStore.setActiveChannel(null);
    d.stores.unreadStore.increment(chId, true);  // hasInteraction=true
    d.stores.unreadStore.increment(chId, false);
  });
  await page.waitForTimeout(120);
  const waiting = await page.evaluate(() => {
    const row = document.querySelector('.v2-attention-row');
    return {
      hasInteractionClass: row?.className.includes('has-interaction'),
      statusLabel: row?.querySelector('.v2-attention-status')?.textContent?.trim(),
      badge: row?.querySelector('.v2-ch-unread-badge')?.textContent?.trim(),
    };
  });
  check('Waiting row has .has-interaction class',       waiting.hasInteractionClass === true);
  check('Waiting row label reads "Needs you"',          /needs you/i.test(waiting.statusLabel || ''), waiting.statusLabel);
  check('Waiting row shows unread count badge',         waiting.badge === '2', waiting.badge);

  // ---- Recent-activity grace window: channel that just stopped
  //      running (no unread, no interaction) should stay visible
  //      with a "Xm" timestamp for up to an hour.
  await page.evaluate(() => {
    const d = window.__v2debug;
    const chId = [...d.stores.channelsStore.list()][0].id;
    d.stores.uiStore.setActiveChannel(chId);
    d.stores.unreadStore.markRead(chId);
    d.stores.presenceStore.setAgentActive(chId, true);
    d.stores.presenceStore.setAgentActive(chId, false);  // stops + stamps lastActiveAt
  });
  await page.waitForTimeout(150);
  const recent = await page.evaluate(() => {
    const row = document.querySelector('.v2-attention-row');
    return {
      hasRecentClass: row?.className.includes('status-recent'),
      statusLabel: row?.querySelector('.v2-attention-status')?.textContent?.trim(),
    };
  });
  check('Recent-activity row uses status-recent',
        recent.hasRecentClass === true, JSON.stringify(recent));
  check('Recent row shows a relative timestamp',
        /(just now|\d+m)/.test(recent.statusLabel || ''),
        recent.statusLabel);

  // Force the lastActiveAt > 1hr → row should disappear.
  await page.evaluate(() => {
    const d = window.__v2debug;
    const chId = [...d.stores.channelsStore.list()][0].id;
    // Reach into the store's slot to push its timestamp out of window.
    const slot = d.stores.presenceStore.get(chId);
    slot.lastActiveAt = Date.now() - (2 * 60 * 60 * 1000);  // 2h ago
    // Trigger a re-render without changing active state.
    d.stores.unreadStore.markRead(chId);
  });
  await page.waitForTimeout(150);
  const aged = await page.evaluate(() => !!document.querySelector('.v2-attention-section'));
  check('Attention section clears once the recent window lapses', aged === false);

  // ---- Rail chat badge: should be HIDDEN because no active channel.
  await page.evaluate(() => window.__v2debug.stores.uiStore.setActiveChannel(null));
  await page.waitForTimeout(80);
  const railHiddenNoActive = await page.evaluate(() => {
    const el = document.getElementById('v2-rail-unread');
    return el?.hidden === true;
  });
  check('Rail chat badge hidden with no active channel', railHiddenNoActive);

  // Select the channel → badge still hidden because the act of
  // activation triggers markRead elsewhere? Actually the rail just
  // reads the current unread slot. Re-add unread for the now-active
  // channel via direct store mutation (bypassing the active-channel
  // suppression).
  await page.evaluate(() => {
    const d = window.__v2debug;
    const chId = [...d.stores.channelsStore.list()][0].id;
    d.stores.uiStore.setActiveChannel(chId);
    // Force-set unread directly even though the channel is active.
    // This exercises the rail's "show for active channel" path.
    d.stores.unreadStore.increment(chId, false);
  });
  await page.waitForTimeout(120);
  const railActive = await page.evaluate(() => {
    const el = document.getElementById('v2-rail-unread');
    return {
      hidden: el?.hidden,
      text: el?.textContent?.trim(),
    };
  });
  check('Rail badge visible when active channel has unread', railActive.hidden === false, JSON.stringify(railActive));
  check('Rail badge shows the active-channel count',          /^\d+$/.test(railActive.text || ''));

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
