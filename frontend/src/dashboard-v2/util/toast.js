// Transient toast notifications. Injected into #v2-toast (set up by
// shell/layout.js). Ported from v1 util/toast.js.

const HOST_ID = 'v2-toast';
const DEFAULT_TTL_MS = 3000;

export function showToast(text, opts = {}) {
  const host = document.getElementById(HOST_ID);
  if (!host) return;
  const kind = opts.kind || 'info';
  const ttl = opts.ttlMs || DEFAULT_TTL_MS;
  const el = document.createElement('div');
  el.className = `v2-toast-item v2-toast-${kind}`;
  el.textContent = text;
  host.appendChild(el);
  // Fade-in on next frame.
  requestAnimationFrame(() => el.classList.add('visible'));
  setTimeout(() => {
    el.classList.remove('visible');
    setTimeout(() => el.remove(), 200);
  }, ttl);
}
