// The inbox's one-tap offer to turn push notifications on (#191).
//
// Settings already has the switch, and nobody found it: prod had no push
// subscription at all. So the rail offers it once, where it is seen, until the
// browser's permission is decided either way — then it is gone for good, even
// if the browser is later reset to ask again — or until the user dismisses it,
// which survives a reload. The ask happens inside the click, because iOS only
// grants a permission prompt that follows a user gesture.
//
// It paints from its cached record like every other view: the record says
// whether the offer was dismissed or settled, and the browser says whether it
// can still be asked. Neither is connection state.

import { enablePush, pushSupported } from "../push.js";
import { uiAddress, watchUiState } from "./localUiState.js";

export const PUSH_PROMPT_ADDRESS = uiAddress({ view: "inbox", kind: "push-prompt" });

/** Whether the offer stands, from the cached record and what the browser says.
 *  Pure. */
export function pushPromptOffered(saved, { supported, permission }) {
  return supported && permission === "default" && !saved?.dismissed && !saved?.settled;
}

const PROMPT_HTML = `
  <p class="push-prompt-text">Get a notification when something needs you.</p>
  <div class="push-prompt-actions">
    <button class="btn primary" type="button" data-push-enable>Turn on</button>
    <button class="iconbtn push-prompt-dismiss" type="button" data-push-dismiss title="Not now" aria-label="Not now">×</button>
  </div>`;

/** Hear the browser's notification permission change — from Settings, or from
 *  the browser's own site settings. Returns the unsubscribe. */
function watchNotificationPermission(listener) {
  let status = null;
  let disposed = false;
  globalThis.navigator?.permissions?.query({ name: "notifications" })
    .then((answer) => {
      if (disposed) return;
      status = answer;
      status.addEventListener("change", listener);
    })
    .catch(() => {});
  return () => {
    disposed = true;
    status?.removeEventListener("change", listener);
  };
}

const BROWSER_PUSH = Object.freeze({
  supported: pushSupported,
  permission: () => globalThis.Notification?.permission || "default",
  enable: enablePush,
  onPermissionChange: watchNotificationPermission,
});

/** Mount the offer on `host` (hidden until its record says otherwise).
 *  `browser` is the push surface, injectable for tests. */
export function mountPushPrompt(host, browser = BROWSER_PUSH) {
  let saved;
  let painted = false;
  const paint = (value) => {
    painted = true;
    saved = value;
    const offered = pushPromptOffered(saved, { supported: browser.supported(), permission: browser.permission() });
    host.hidden = !offered;
    host.innerHTML = offered ? PROMPT_HTML : "";
  };
  const record = watchUiState(PUSH_PROMPT_ADDRESS, paint);
  const decided = () => browser.supported() && browser.permission() !== "default";
  const settleIfDecided = () => (decided() && !saved?.settled ? record.write({ ...saved, settled: true }) : Promise.resolve());
  host.onclick = (event) => {
    if (event.target.closest("[data-push-dismiss]")) {
      void record.write({ ...saved, dismissed: true });
      return;
    }
    if (!event.target.closest("[data-push-enable]")) return;
    // Called straight from the click: the permission prompt must follow the
    // gesture. A subscription that fails after a grant is Settings' to retry.
    browser.enable()
      .catch((error) => console.warn("push: could not turn notifications on", error))
      .finally(() => void settleIfDecided());
  };
  const stopHearing = browser.onPermissionChange(() => void settleIfDecided());
  const ready = record.ready.then(() => {
    if (!painted) paint(undefined);
    return settleIfDecided();
  });
  return {
    ready,
    dispose() {
      stopHearing();
      host.onclick = null;
      record.dispose();
    },
  };
}
