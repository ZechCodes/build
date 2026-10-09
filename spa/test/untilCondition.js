// Waits on the condition a case depends on, not on a budget of turns or
// milliseconds. Under load a fixed flush count runs out before a cache read
// lands, and a polling wait's one-second default runs out before a starved
// worker gets there; both fail cases whose product did nothing wrong. These
// resolve when the thing itself happens, and the test timeout is the only
// bound left (#430).

/** Resolves with the predicate's first truthy answer: now, or after whichever
 *  change to the document makes it so. */
export function untilDom(predicate, root = document) {
  const now = predicate();
  if (now) return Promise.resolve(now);
  return new Promise((resolve, reject) => {
    const observer = new MutationObserver(() => {
      let answer;
      try {
        answer = predicate();
      } catch (error) {
        observer.disconnect();
        reject(error);
        return;
      }
      if (!answer) return;
      observer.disconnect();
      resolve(answer);
    });
    observer.observe(root, { attributes: true, characterData: true, childList: true, subtree: true });
  });
}

/** Resolves once a `vi.fn()`'s calls satisfy `enough` — a count, or a test of
 *  the calls array — at once when they already do. Its implementation is kept. */
export function untilCalled(mock, enough = 1) {
  const satisfied = typeof enough === "number" ? (calls) => calls.length >= enough : enough;
  if (satisfied(mock.mock.calls)) return Promise.resolve(mock.mock.calls);
  return new Promise((resolve) => {
    const implementation = mock.getMockImplementation();
    mock.mockImplementation((...args) => {
      try {
        return implementation ? implementation(...args) : undefined;
      } finally {
        if (satisfied(mock.mock.calls)) {
          mock.mockImplementation(implementation);
          resolve(mock.mock.calls);
        }
      }
    });
  });
}

/** Resolves with `check`'s first truthy answer: now, or after whichever cache
 *  write or eviction at or under `prefixAddress` makes it so. Takes the
 *  module's own `subscribeCache`, since a case that resets modules holds a
 *  cache of its own. */
export function untilCache(subscribeCache, prefixAddress, check) {
  return new Promise((resolve, reject) => {
    let settled = false;
    let checking = Promise.resolve();
    let stop = () => {};
    const recheck = () => {
      checking = checking.then(async () => {
        if (settled) return;
        const answer = await check();
        if (!answer || settled) return;
        settled = true;
        stop();
        resolve(answer);
      }).catch((error) => {
        settled = true;
        stop();
        reject(error);
      });
    };
    // Listening before the first look, so a write between the two is heard.
    stop = subscribeCache(prefixAddress, recheck);
    recheck();
  });
}

/** Resolves with `check`'s first truthy answer: now, or after whichever
 *  notice from `subscribe(listener)` (which answers its unsubscribe) makes
 *  it so. */
export function untilHeard(subscribe, check) {
  const now = check();
  if (now) return Promise.resolve(now);
  return new Promise((resolve) => {
    const stop = subscribe(() => {
      const answer = check();
      if (!answer) return;
      stop();
      resolve(answer);
    });
  });
}
