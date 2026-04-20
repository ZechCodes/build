let _timer = null;

export function showToast(msg, ms) {
  const el = document.getElementById('ui-toast');
  if (!el) return;
  el.textContent = msg;
  el.classList.add('show');
  clearTimeout(_timer);
  _timer = setTimeout(() => el.classList.remove('show'), ms || 2000);
}
