// Dashboard v2 — bootstrap.
//
// Wave 3: stores + transport + shell. The shell builds the layout
// imperatively and owns the sidebar, tab bar, top bar, and rail. Channel
// views come in Wave 4.

import { log } from './core/log.js';
import { bus } from './core/bus.js';
import { sessionStore, initSessionStore } from './core/session-store.js';
import {
  initStores,
  devicesStore, channelsStore, messagesStore, activityStore,
  presenceStore, unreadStore, filesStore, terminalStore,
  tasksStore, complicationsStore, uiStore, currentToolStore,
} from './domain/index.js';
import { initTransport, e2eePool } from './transport/index.js';
import { initShell } from './shell/app.js';
import { router } from './shell/router.js';
import { channelRegistry } from './channel/registry.js';

const plog = log('bootstrap');

function exposeDebug() {
  window.__v2debug = {
    bus,
    router,
    channelRegistry,
    sessionStore,
    stores: {
      devicesStore, channelsStore, messagesStore, activityStore,
      presenceStore, unreadStore, filesStore, terminalStore,
      tasksStore, complicationsStore, uiStore, currentToolStore,
    },
    e2eePool,
  };
}

async function boot() {
  const root = document.getElementById('app');
  if (!root) {
    plog.error('no #app root found');
    return;
  }
  initStores();
  // Session store must be initialized BEFORE transport binds its
  // handlers, so it sees every sse/e2ee event from the very first
  // tick. Shell views + channel registry come afterward and read
  // the session phase off it.
  initSessionStore({ devicesStore, channelsStore, uiStore });
  exposeDebug();
  initShell(root);
  channelRegistry.init();
  try {
    await initTransport();
  } catch (err) {
    plog.error('transport init failed', err);
  }
  plog.info('ready');
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', boot);
} else {
  boot();
}
