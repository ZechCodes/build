// Waits on the condition a test reads, for as long as the work before it
// takes. `vi.waitFor` polls against a one-second deadline, and on a loaded
// machine the IndexedDB writes ahead of a call or a paint can outlast that;
// these settle when the thing has happened, and only the test's own timeout
// bounds them.

/** Settles once `until(root)` holds: now, or after the DOM under `root`
 *  changes. Answers what `until` answered. */
export function painted(until, root = document.body) {
  return new Promise((resolve) => {
    const observer = new MutationObserver(() => check());
    const check = () => {
      const found = until(root);
      if (!found) return false;
      observer.disconnect();
      resolve(found);
      return true;
    };
    if (!check()) observer.observe(root, { subtree: true, childList: true, attributes: true, characterData: true });
  });
}

/** Settles once `until(mock)` holds: now, or as one of the mock's calls is
 *  made. By default, once it has been called at all. */
export function called(mock, until = () => mock.mock.calls.length > 0) {
  return new Promise((resolve) => {
    if (until(mock)) return resolve();
    const original = mock.getMockImplementation();
    mock.mockImplementation(function (...args) {
      const result = original?.apply(this, args);
      if (until(mock)) {
        mock.mockImplementation(original || (() => undefined));
        resolve();
      }
      return result;
    });
  });
}

/** Settles once the async `until()` holds: now, or after one of the
 *  announcements `watch(heard)` subscribes to (a cache or UI record's). */
export function announced(watch, until) {
  return new Promise((resolve) => {
    let done = false;
    let stop = () => {};
    const check = async () => {
      if (done || !(await until())) return;
      done = true;
      stop();
      resolve();
    };
    stop = watch(() => void check());
    void check();
  });
}

/** `until` for the waits above, from an assertion: holds once it passes. An
 *  async assertion (for `announced`) answers a promise of the same. */
export const holds = (assertion) => () => {
  try {
    const checked = assertion();
    return checked instanceof Promise ? checked.then(() => true, () => false) : true;
  } catch {
    return false;
  }
};

/** Settles once no cache sync pass is out on `deviceId` (`sync` is the
 *  cacheSync module the test imported), including one asked for while
 *  another ran: an ask made mid-pass is folded into it, not read again. */
export async function passesSettled(sync, deviceId) {
  for (let pass = sync.passInFlight(deviceId); pass; pass = sync.passInFlight(deviceId)) await pass;
}
