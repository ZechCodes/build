export function initNotifications() {
  if (window.__skriftNotifications) {
    window.__skriftNotifications.configure({ persistConnection: true });
    // Patch _healthCheck to clear _hiddenSince in the CLOSED branch,
    // preventing a double reconnect when both visibilitychange and focus fire.
    const sn = window.__skriftNotifications;
    const origHealthCheck = sn._healthCheck.bind(sn);
    sn._healthCheck = function () {
      if (this._es && this._es.readyState === EventSource.CLOSED) {
        this._hiddenSince = null;
      }
      return origHealthCheck();
    }.bind(sn);
  } else {
    setTimeout(initNotifications, 100);
  }
}

initNotifications();

/** True when the SSE connection has completed sync (replay phase is over). */
export function sseSynced() {
  return window.__skriftNotifications?._synced === true;
}
