// Dashboard v2 — bootstrap.
//
// Waves 0–2 scaffold: still rendering a placeholder. Wave 2 wires stores
// and transport so the placeholder is fed by live data (devtools-visible
// only — no UI yet). See planning/dashboard-v2/07-migration.md.

// styles are emitted by the separate `css/dashboard-v2` esbuild entry — the
// template loads them via <link>. Don't import CSS here or esbuild will also
// inline a copy next to the JS bundle.
import { log } from './core/log.js';
import { bus } from './core/bus.js';
import {
  initStores,
  devicesStore, channelsStore, messagesStore, activityStore,
  presenceStore, unreadStore, filesStore, terminalStore,
  tasksStore, complicationsStore, uiStore,
} from './domain/index.js';
import { initTransport, e2eePool } from './transport/index.js';

const plog = log('bootstrap');

function renderPlaceholder(root) {
  root.innerHTML = `
    <div class="v2-placeholder">
      <div class="v2-placeholder-card">
        <div class="v2-placeholder-label">Dashboard v2 · Wave 2</div>
        <h1>Transport online</h1>
        <p>Domain stores are live and listening to the bus. No UI yet — open
          devtools and inspect <code>window.__v2debug</code> to watch stores
          populate as E2EE frames arrive.</p>
        <ul class="v2-placeholder-checklist">
          <li data-state="done">Route <code>/dashboard-v2/</code></li>
          <li data-state="done">Template <code>dashboard_v2.html</code></li>
          <li data-state="done">Bundle <code>dashboard-v2.js</code> / <code>.css</code></li>
          <li data-state="done">Planning docs under <code>planning/dashboard-v2/</code></li>
          <li data-state="done">Wave 1 — core + domain stores</li>
          <li data-state="done">Wave 2 — transport + dispatcher</li>
          <li data-state="pending">Wave 3 — shell + channel panel</li>
          <li data-state="pending">Wave 4 — channel controller + ChatView + ConsoleView</li>
          <li data-state="pending">Wave 5 — FilesView + TerminalView</li>
          <li data-state="pending">Wave 6 — remaining views + polish</li>
          <li data-state="pending">Wave 7 — cutover</li>
        </ul>
      </div>
    </div>
  `;
}

function exposeDebug() {
  // Dev-only handle for console inspection. Prod builds strip this via
  // esbuild `define` once a production flag is added — for now, always on
  // since v2 only ships behind the /dashboard-v2/ route.
  window.__v2debug = {
    bus,
    stores: {
      devicesStore, channelsStore, messagesStore, activityStore,
      presenceStore, unreadStore, filesStore, terminalStore,
      tasksStore, complicationsStore, uiStore,
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
  renderPlaceholder(root);
  exposeDebug();
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
