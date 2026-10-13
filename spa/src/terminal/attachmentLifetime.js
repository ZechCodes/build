// Pane cancellation follows attachment through connection, source validation
// and the reply. It never changes the lifetime of the server shell.
export function assertAttachmentCurrent({ signal, isCurrent = () => true } = {}) {
  if (signal?.aborted || !isCurrent()) throw new DOMException("This terminal pane closed.", "AbortError");
}

export function withAttachmentCancellation(promise, signal) {
  if (!signal) return promise;
  return new Promise((resolve, reject) => {
    const abort = () => reject(new DOMException("This terminal pane closed.", "AbortError"));
    const settle = (callback) => (value) => {
      signal.removeEventListener("abort", abort);
      callback(value);
    };
    // Keep observing the underlying request even after abort so a late refusal
    // is consumed; the caller will never apply its result to the disposed pane.
    Promise.resolve(promise).then(settle(resolve), settle(reject));
    if (signal.aborted) abort();
    else signal.addEventListener("abort", abort, { once: true });
  });
}
