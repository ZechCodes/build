// Dashboard v2 — bootstrap.
//
// Wave 0 scaffold: renders a placeholder so the parallel route is visible.
// Wave 1+ will wire stores, transport, shell in order. See
// planning/dashboard-v2/07-migration.md.

// styles are emitted by the separate `css/dashboard-v2` esbuild entry — the
// template loads them via <link>. Don't import CSS here or esbuild will also
// inline a copy next to the JS bundle.
import { log } from './core/log.js';
import { initStores } from './domain/index.js';

const plog = log('bootstrap');

function renderPlaceholder(root) {
  root.innerHTML = `
    <div class="v2-placeholder">
      <div class="v2-placeholder-card">
        <div class="v2-placeholder-label">Dashboard v2 · Wave 0</div>
        <h1>Scaffolded</h1>
        <p>The parallel rewrite is wired. No domain code yet — see
          <code>planning/dashboard-v2/</code> for the plan.</p>
        <ul class="v2-placeholder-checklist">
          <li data-state="done">Route <code>/dashboard-v2/</code></li>
          <li data-state="done">Template <code>dashboard_v2.html</code></li>
          <li data-state="done">Bundle <code>dashboard-v2.js</code> / <code>.css</code></li>
          <li data-state="done">Planning docs under <code>planning/dashboard-v2/</code></li>
          <li data-state="pending">Wave 1 — core + domain stores</li>
          <li data-state="pending">Wave 2 — transport + dispatcher</li>
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

function boot() {
  const root = document.getElementById('app');
  if (!root) {
    plog.error('no #app root found');
    return;
  }
  initStores();
  renderPlaceholder(root);
  plog.info('ready');
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', boot);
} else {
  boot();
}
