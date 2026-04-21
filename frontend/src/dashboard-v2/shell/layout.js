// Top-level DOM scaffold. Matches v1 dashboard.html visual structure:
// sidebar + viewer (files-layout) + complications + bottom rail + floating
// chat overlay. No tab bar.
//
// Every slot named here is where a specific view mounts. No per-channel or
// per-view logic in this file; it just carves out the regions.

export function buildLayout(root) {
  root.innerHTML = `
    <div class="v2-app" data-rail="collapsed" data-overlay="closed">

      <!-- ===== SIDEBAR ===== -->
      <aside class="v2-sidebar" id="v2-sidebar">
        <div class="v2-sidebar-inner">
          <section class="v2-sidebar-section" data-section="devices">
            <header class="v2-sidebar-section-header" data-section-static>
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
              <span class="v2-tasks-badge-slot" id="v2-tasks-badge-slot"></span>
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

      <!-- ===== MAIN VIEWER (always Files) ===== -->
      <main class="v2-viewer" id="v2-viewer">
        <header class="v2-path-bar" id="v2-path-bar">
          <span class="v2-path-text empty" id="v2-path-text">No file selected</span>
        </header>
        <div class="v2-files-main" id="v2-viewer-main">
          <aside class="v2-files-tree-panel" id="v2-files-tree-panel"></aside>
          <section class="v2-files-content-panel" id="v2-files-content-panel"></section>
        </div>
        <div class="v2-complications empty" id="v2-complications"></div>
      </main>

      <!-- ===== BOTTOM RAIL (Terminal + Chat toggles, drag, body) ===== -->
      <footer class="v2-rail" id="v2-rail">
        <div class="v2-rail-drag" id="v2-rail-drag" title="Drag to resize"></div>
        <div class="v2-rail-header" id="v2-rail-controls">
          <div class="v2-rail-group v2-rail-group-left">
            <button class="v2-rail-btn" id="v2-rail-terminal-toggle" data-rail-panel="terminal" type="button" title="Toggle terminal">
              <svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><rect x="1.5" y="2.5" width="13" height="11" rx="1.5" stroke="currentColor" stroke-width="1.3"/><path d="M4 6l2 2-2 2M7.5 10.5H11" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/></svg>
              <span>Terminal</span>
            </button>
          </div>
          <div class="v2-rail-group v2-rail-group-spacer"></div>
          <div class="v2-rail-group v2-rail-group-right">
            <span class="v2-rail-status" id="v2-rail-status">Connected</span>
            <button class="v2-rail-btn" id="v2-rail-chat-toggle" type="button" title="Toggle chat">
              <svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M2 3.5a1 1 0 011-1h10a1 1 0 011 1v7a1 1 0 01-1 1H6l-3 2.5V11.5a1 1 0 01-1-1v-7z" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/></svg>
              <span>Chat</span>
              <span class="v2-rail-unread" id="v2-rail-unread" hidden></span>
            </button>
          </div>
        </div>
        <div class="v2-rail-body" id="v2-rail-body"></div>
      </footer>

      <!-- ===== FLOATING CHAT OVERLAY ===== -->
      <div class="v2-chat-overlay" id="v2-chat-overlay" aria-hidden="true">
        <header class="v2-co-header" id="v2-co-header">
          <span class="v2-co-title">
            <span class="v2-co-hash">#</span>
            <span class="v2-co-channel-name" id="v2-co-channel-name">Select a channel</span>
          </span>
          <div class="v2-co-actions">
            <button class="v2-co-btn" id="v2-co-pin" type="button" title="Pin open">
              <svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M10 2v4l3 2v2H3V8l3-2V2h4z" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/><path d="M8 10v4" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/></svg>
            </button>
            <button class="v2-co-btn" id="v2-co-expand" type="button" title="Expand">
              <svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M9 2h5v5M7 14H2V9" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/><path d="M14 2l-5 5M2 14l5-5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/></svg>
            </button>
            <button class="v2-co-btn" id="v2-co-minimize" type="button" title="Minimize">
              <svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M3 8h10" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>
            </button>
          </div>
        </header>
        <div class="v2-co-body" id="v2-chat-overlay-body"></div>
        <div class="v2-co-toolbar" id="v2-chat-overlay-toolbar"></div>
      </div>

      <div class="v2-dropdown-layer" id="v2-dropdown-layer"></div>
      <div class="v2-toast" id="v2-toast" aria-live="polite"></div>
    </div>
  `;
}
