const coarsePointer = () => window.matchMedia?.("(pointer: coarse)").matches === true;

/** Reveal one delegated target after a stationary press. Pointer taps and
 * scrolling keep their defaults; only the click following a hold is consumed.
 * Targets and presentation belong to the caller, so this also works for lists
 * other than the Files tree. */
export function mountLongPress(host, { targetOf, onReveal, onDismiss, canStart = () => true, enabled = coarsePointer, delay = 500, tolerance = 6 }) {
  const document = host.ownerDocument;
  let press = null;
  let revealed = null;
  let heldClick = null;
  const targetOfEvent = (event) => host.contains(event.target) ? targetOf(event) : null;
  const cancel = () => {
    clearTimeout(press?.timer);
    press = null;
  };
  const dismiss = () => {
    if (revealed) onDismiss(revealed);
    revealed = null;
  };
  const clear = () => {
    cancel();
    dismiss();
    heldClick = null;
  };
  const reveal = () => {
    const target = press.target;
    if (!target.isConnected || !host.contains(target)) return cancel();
    revealed = target;
    heldClick = target;
    onReveal(target);
  };
  const onDown = (event) => {
    cancel();
    heldClick = null;
    if (!enabled()) return;
    const target = targetOfEvent(event);
    if (target !== revealed) dismiss();
    if (!target || event.button !== 0 || event.isPrimary === false || !canStart(event)) return;
    press = { target, id: event.pointerId, x: event.clientX, y: event.clientY, timer: setTimeout(reveal, delay) };
  };
  const onMove = (event) => {
    if (!press || event.pointerId !== press.id) return;
    if (Math.hypot(event.clientX - press.x, event.clientY - press.y) > tolerance) cancel();
  };
  const onEnd = (event) => {
    if (event.pointerId === press?.id) cancel();
  };
  const onMenu = (event) => {
    const target = targetOfEvent(event);
    if (!target) return;
    // Some touch engines emit their native menu after pointerup. Keep that
    // release covered without intercepting later mouse or keyboard menus.
    if (target === press?.target || (event.pointerType === "touch" && target === heldClick)) event.preventDefault();
  };
  const onClick = (event) => {
    if (!heldClick || event.detail === 0 || targetOfEvent(event) !== heldClick) return;
    heldClick = null;
    event.preventDefault();
    event.stopImmediatePropagation();
  };
  const listeners = { pointerdown: onDown, pointermove: onMove, pointerup: onEnd, pointercancel: onEnd, contextmenu: onMenu, click: onClick, scroll: cancel };
  Object.entries(listeners).forEach(([type, listener]) => document.addEventListener(type, listener, true));
  return {
    clear,
    dispose() {
      clear();
      Object.entries(listeners).forEach(([type, listener]) => document.removeEventListener(type, listener, true));
    },
  };
}
