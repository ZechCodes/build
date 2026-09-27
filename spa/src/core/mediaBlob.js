// Media is decoded one cached byte page at a time. The browser owns the Blob's
// byte storage; no whole-file base64 string or data URL is made on the way.
import { bytesOfBase64 } from "./bodyPages.js";

const attached = new Map();
const observers = new WeakMap();

const pageBody = (page) => typeof page === "string" ? page : page?.body || "";

export function createMediaUrl(pages, mime) {
  const parts = pages.map((page) => bytesOfBase64(pageBody(page)));
  const url = URL.createObjectURL(new Blob(parts, { type: mime || "application/octet-stream" }));
  return { url, revoke: () => URL.revokeObjectURL(url) };
}

export function releaseMediaSource(element) {
  const release = attached.get(element);
  if (release) {
    attached.delete(element);
    release();
  }
}

function watchRemoved(doc) {
  if (observers.has(doc)) return;
  const observer = new MutationObserver(() => {
    for (const element of attached.keys()) {
      if (element.ownerDocument === doc && !element.isConnected) releaseMediaSource(element);
    }
  });
  observer.observe(doc, { childList: true, subtree: true });
  observers.set(doc, observer);
}

export function attachMediaSource(element, pages, mime) {
  releaseMediaSource(element);
  const held = createMediaUrl(pages, mime);
  attached.set(element, held.revoke);
  element.setAttribute("src", held.url);
  watchRemoved(element.ownerDocument);
  return held.url;
}
