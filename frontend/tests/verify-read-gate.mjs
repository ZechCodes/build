// Verify scroll-gated read receipts:
//   - active-channel incoming messages remain unread on arrival,
//   - interaction before the message bottom is reached does not mark read,
//   - scrolling to/past the message bottom marks read and seen.
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8092';
const EMAIL = process.env.EMAIL || 'read-gate@test.local';

const browser = await chromium.launch({ headless: true });
const ctx = await browser.newContext({ viewport: { width: 1280, height: 820 } });
const page = await ctx.newPage();

let passed = 0, failed = 0;
function check(label, ok, note) {
  if (ok) { passed++; console.log(`[PASS] ${label}${note ? ` - ${note}` : ''}`); }
  else    { failed++; console.log(`[FAIL] ${label}${note ? ` - ${note}` : ''}`); }
}

try {
  await page.goto(`${BASE}/auth/dummy/login`, { waitUntil: 'domcontentloaded' });
  await page.fill('input[name="email"]', EMAIL);
  const name = await page.$('input[name="name"]');
  if (name) await page.fill('input[name="name"]', 'Read Gate');
  await Promise.all([
    page.waitForURL(u => !u.pathname.startsWith('/auth/'), { timeout: 10000 }).catch(() => null),
    page.click('button[type="submit"]'),
  ]);

  await page.goto(`${BASE}/dashboard/`, { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.v2-app', { timeout: 10000 });

  await page.evaluate(() => {
    const d = window.__v2debug;
    window.__readGateEvents = { markRead: [], markSeen: [] };
    d.bus.on('intent.mark_read', p => window.__readGateEvents.markRead.push(p));
    d.bus.on('intent.mark_seen', p => window.__readGateEvents.markSeen.push(p));
    d.bus.on('intent.get_messages', p => {
      if (p.channelId === 'ch-read-gate') d.bus.emit('message.bulk', { channelId: 'ch-read-gate', msgs: [] });
    });
    d.bus.on('intent.files_list', p => {
      if (p.channelId === 'ch-read-gate') d.bus.emit('files.list_result', { channelId: 'ch-read-gate', path: '', entries: [] });
    });
    d.bus.emit('device.bulk', {
      devices: [{ id: 'dev-read-gate', name: 'Read Device', status: 'online', has_transport_key: true }],
    });
    d.bus.emit('channel.list', {
      deviceId: 'dev-read-gate',
      channels: [{ id: 'ch-read-gate', name: 'read-gate', created_at: Date.now() }],
    });
    d.bus.emit('sse.connected', {});
    d.bus.emit('e2ee.connected', { deviceId: 'dev-read-gate' });
  });
  await page.waitForSelector('.v2-channel-sidebar-item[data-channel-id="ch-read-gate"]', { timeout: 5000 });
  await page.click('.v2-channel-sidebar-item[data-channel-id="ch-read-gate"]');
  await page.waitForFunction(() => window.__v2debug.stores.uiStore.getActiveChannel() === 'ch-read-gate');
  // The verifier drives messages directly through the bus rather than
  // opening an E2EE transport, so expose the mounted chat pane without
  // waiting on the full channel-loader lifecycle.
  await page.evaluate(() => {
    window.__v2debug.stores.uiStore.setOverlayOpen(true);
    document.querySelector('.v2-app').dataset.channelPhase = 'ready';
    document.querySelector('#v2-chat-overlay').style.height = '260px';
  });
  await page.waitForSelector('.v2-chat-messages', { state: 'visible', timeout: 5000 });

  const longBody = Array.from({ length: 220 }, (_, i) => `Line ${i + 1}: unread content that requires scrolling.`).join('\n\n');
  await page.evaluate((content) => {
    window.__v2debug.bus.emit('message.received', {
      channelId: 'ch-read-gate',
      msg: {
        id: 'm-read-gate',
        channel_id: 'ch-read-gate',
        sender: 'Agent',
        content,
        created_at: new Date().toISOString(),
        read_at: null,
      },
    });
  }, longBody);
  await page.waitForSelector('[data-msg-id="m-read-gate"]', { timeout: 5000 });
  await page.waitForTimeout(200);
  await page.evaluate(() => {
    const msg = document.querySelector('[data-msg-id="m-read-gate"]');
    const scroller = document.querySelector('.v2-chat-messages');
    msg.style.minHeight = '900px';
    scroller.scrollTop = 0;
  });
  await page.waitForTimeout(320);

  const afterArrival = await page.evaluate(() => {
    const slot = window.__v2debug.stores.unreadStore.get('ch-read-gate');
    const msg = document.querySelector('[data-msg-id="m-read-gate"]');
    const scroller = document.querySelector('.v2-chat-messages');
    return {
      count: slot.count,
      markRead: window.__readGateEvents.markRead.length,
      markSeen: window.__readGateEvents.markSeen.length,
      atBottom: msg.offsetTop + msg.offsetHeight <= scroller.scrollTop + scroller.clientHeight + 1,
    };
  });
  check('active incoming message remains unread on arrival',
    afterArrival.count === 1 && afterArrival.markRead === 0 && afterArrival.markSeen === 0,
    JSON.stringify(afterArrival));

  await page.mouse.move(300, 300);
  await page.waitForTimeout(120);
  const beforeBottom = await page.evaluate(() => ({
    count: window.__v2debug.stores.unreadStore.get('ch-read-gate').count,
    markRead: window.__readGateEvents.markRead.length,
    markSeen: window.__readGateEvents.markSeen.length,
  }));
  check('post-delay interaction alone does not mark read before message bottom',
    beforeBottom.count === 1 && beforeBottom.markRead === 0 && beforeBottom.markSeen === 0,
    JSON.stringify(beforeBottom));

  const box = await page.locator('.v2-chat-messages').boundingBox();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.wheel(0, 8000);
  await page.waitForTimeout(350);

  const afterScroll = await page.evaluate(() => {
    const msg = window.__v2debug.stores.messagesStore.forChannel('ch-read-gate')[0];
    const slot = window.__v2debug.stores.unreadStore.get('ch-read-gate');
    return {
      readAt: !!msg.read_at,
      count: slot.count,
      markRead: window.__readGateEvents.markRead.length,
      markReadIds: window.__readGateEvents.markRead.flatMap(e => e.msgIds || []),
      markSeen: window.__readGateEvents.markSeen.length,
    };
  });
  check('scrolling to message bottom marks message read',
    afterScroll.readAt === true && afterScroll.count === 0 && afterScroll.markRead === 1
      && afterScroll.markReadIds.includes('m-read-gate'),
    JSON.stringify(afterScroll));
  check('latest visible message marks channel seen',
    afterScroll.markSeen === 1,
    JSON.stringify(afterScroll));

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
