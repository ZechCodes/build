// Top-level DOM scaffold. See planning/dashboard-v2/06-shell.md.
//
// Every slot named here is where a specific view mounts. No per-channel or
// per-view logic in this file; it just carves out the regions.

export function buildLayout(root) {
  root.innerHTML = `
    <div class="v2-app" data-rail="collapsed">
      <aside class="v2-sidebar" id="v2-sidebar">
        <div class="v2-sidebar-inner">
          <section class="v2-sidebar-section" data-section="devices">
            <header class="v2-sidebar-section-header" data-sidebar-toggle="devices">
              <svg class="v2-chev" viewBox="0 0 12 12" aria-hidden="true"><path d="M4 2l4 4-4 4" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round" stroke-linejoin="round"/></svg>
              <span>Devices</span>
            </header>
            <div class="v2-sidebar-section-body">
              <div id="v2-channel-panel-list" class="v2-channel-panel-list"></div>
            </div>
          </section>
          <section class="v2-sidebar-section" data-section="tasks">
            <header class="v2-sidebar-section-header" data-sidebar-toggle="tasks">
              <svg class="v2-chev" viewBox="0 0 12 12" aria-hidden="true"><path d="M4 2l4 4-4 4" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round" stroke-linejoin="round"/></svg>
              <span>Tasks</span>
            </header>
            <div class="v2-sidebar-section-body" id="v2-tasks-body"></div>
          </section>
          <section class="v2-sidebar-section collapsed" data-section="activity">
            <header class="v2-sidebar-section-header" data-sidebar-toggle="activity">
              <svg class="v2-chev" viewBox="0 0 12 12" aria-hidden="true"><path d="M4 2l4 4-4 4" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round" stroke-linejoin="round"/></svg>
              <span>Activity</span>
            </header>
            <div class="v2-sidebar-section-body" id="v2-activity-body"></div>
          </section>
        </div>
        <footer class="v2-sidebar-footer" id="v2-account-slot"></footer>
      </aside>
      <main class="v2-viewer" id="v2-viewer">
        <header class="v2-top-bar" id="v2-top-bar"></header>
        <nav class="v2-tab-bar" id="v2-tab-bar"></nav>
        <section class="v2-tab-panels" id="v2-tab-panels">
          <div class="v2-tab-panel" data-tab="files" id="v2-tab-files">
            <div class="v2-empty-viewer">No channel selected. Pick one from the sidebar.</div>
          </div>
          <div class="v2-tab-panel" data-tab="chat" id="v2-tab-chat"></div>
          <div class="v2-tab-panel" data-tab="browser" id="v2-tab-browser"></div>
        </section>
      </main>
      <footer class="v2-rail" id="v2-rail">
        <div class="v2-rail-controls" id="v2-rail-controls"></div>
        <div class="v2-rail-body" id="v2-rail-body"></div>
      </footer>
      <div class="v2-chat-overlay" id="v2-chat-overlay" aria-hidden="true"></div>
      <div class="v2-dropdown-layer" id="v2-dropdown-layer"></div>
    </div>
  `;
}
