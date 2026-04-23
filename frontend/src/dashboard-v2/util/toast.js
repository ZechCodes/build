// Transient toast notifications. Injected into #v2-toast (set up by
// shell/layout.js). Ported from v1 util/toast.js.

const HOST_ID = 'v2-toast';
const DEFAULT_TTL_MS = 3000;
const ERROR_TTL_MS   = 8000;

export function showToast(text, opts = {}) {
  const host = document.getElementById(HOST_ID);
  if (!host) return;
  const kind = opts.kind || 'info';
  // Errors stay on screen longer so the user has time to read (and
  // the click-to-dismiss gives them as much time as they need).
  const ttl = opts.ttlMs || (kind === 'error' ? ERROR_TTL_MS : DEFAULT_TTL_MS);
  const el = document.createElement('div');
  el.className = `v2-toast-item v2-toast-${kind}`;
  el.textContent = text;
  el.setAttribute('role', kind === 'error' ? 'alert' : 'status');
  el.title = 'Click to dismiss';
  host.appendChild(el);
  requestAnimationFrame(() => el.classList.add('visible'));

  let timer;
  const dismiss = () => {
    clearTimeout(timer);
    el.classList.remove('visible');
    setTimeout(() => el.remove(), 200);
  };
  el.addEventListener('click', dismiss);
  timer = setTimeout(dismiss, ttl);
}
