import { THEME_VERSION_KEY } from '../state.js';
import { compareVersions } from '../util/format.js';

function showUpdateBadge() {
  const footer = document.querySelector('.channel-panel-footer');
  if (!footer || footer.querySelector('.theme-update-badge')) return;
  const badge = document.createElement('button');
  badge.className = 'theme-update-badge';
  badge.title = 'New version available — click to reload';
  badge.setAttribute('aria-label', 'Update available, click to reload');
  badge.textContent = 'Click to Update';
  badge.addEventListener('click', () => location.reload());
  footer.appendChild(badge);
}

async function checkThemeVersion() {
  try {
    const res = await fetch('/api/theme/version');
    if (!res.ok) return;
    const { version } = await res.json();
    if (!version) return;
    const stored = localStorage.getItem(THEME_VERSION_KEY);
    if (!stored) {
      localStorage.setItem(THEME_VERSION_KEY, version);
      return;
    }
    if (compareVersions(version, stored) > 0) {
      showUpdateBadge();
      localStorage.setItem(THEME_VERSION_KEY, version);
    }
  } catch (err) {
    console.warn('[Theme] version check failed:', err);
  }
}

// Check on every SSE (re)connect — treats reconnects as potential deploys.
document.addEventListener('sk:notification-status', (evt) => {
  if (evt.detail.status === 'connected') checkThemeVersion();
});
