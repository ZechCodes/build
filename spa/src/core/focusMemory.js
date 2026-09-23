import { uiAddress, watchUiState } from "./localUiState.js";

const selectorFor = (element, root) => {
  const steps = [];
  for (let node = element; node && node !== root; node = node.parentElement) {
    if (!node.parentElement) return null;
    steps.unshift(`${node.localName}:nth-child(${[...node.parentElement.children].indexOf(node) + 1})`);
  }
  return steps.join(" > ");
};

/** Remember the focused control for one route. The read can finish before an
 * async view has built that control, so observe the mount until it appears.
 * A focus move at any time after mount cancels restoration for this session. */
export function mountFocusMemory(root, routeKey, { deviceId = "", entityId = "" } = {}) {
  const doc = root.ownerDocument;
  const address = uiAddress({ deviceId, entityId, view: routeKey, kind: "focus" });
  let moved = false;
  let wanted = null;
  let restored = false;
  const restore = () => {
    if (!wanted || moved || restored || !root.isConnected) return;
    if (doc.activeElement !== doc.body && doc.activeElement !== root) return;
    const target = root.querySelector(wanted);
    if (!target) return;
    restored = true;
    target.focus();
  };
  const record = watchUiState(address, (value) => {
    wanted = typeof value?.selector === "string" ? value.selector : null;
    restore();
  });
  const onFocus = (event) => {
    moved = true;
    if (!root.contains(event.target)) return;
    const selector = selectorFor(event.target, root);
    if (selector) void record.write({ selector });
  };
  doc.addEventListener("focusin", onFocus, true);
  const observer = new MutationObserver(restore);
  observer.observe(root, { childList: true, subtree: true });
  void record.ready.then(restore);
  const dispose = () => {
    observer.disconnect();
    doc.removeEventListener("focusin", onFocus, true);
    record.dispose();
  };
  dispose.settled = record.settled;
  return dispose;
}
