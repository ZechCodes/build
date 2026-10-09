export function whenDom(host, condition) {
  if (condition()) return Promise.resolve();
  return new Promise((resolve) => {
    const observer = new MutationObserver(() => {
      if (!condition()) return;
      observer.disconnect();
      resolve();
    });
    observer.observe(host, { childList: true, subtree: true, attributes: true, characterData: true });
  });
}

export function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
