// Media is decoded one cached byte page at a time. The browser owns the Blob's
// byte storage; no whole-file base64 string or data URL is made on the way.
import { bytesOfBase64 } from "./bodyPages.js";

const attached = new Map();
const observers = new WeakMap();

const pageBody = (page) => typeof page === "string" ? page : page?.body || "";

/** One body owns one Blob, even when several tiles or previews need URLs. */
export function createMediaBody(pages, mime, onBuilt = () => {}) {
  return { pages, mime: mime || "application/octet-stream", blob: null, onBuilt };
}

function blobOf(body) {
  if (body.blob) return body.blob;
  let blob = null;
  for (const page of body.pages) {
    const bytes = bytesOfBase64(pageBody(page));
    blob = new Blob(blob ? [blob, bytes] : [bytes], { type: body.mime });
  }
  body.blob = blob || new Blob([], { type: body.mime });
  body.pages.length = 0;
  body.pages = null;
  body.onBuilt();
  return body.blob;
}

export function createMediaUrl(pagesOrBody, mime) {
  const body = Array.isArray(pagesOrBody) ? createMediaBody(pagesOrBody, mime) : pagesOrBody;
  const url = URL.createObjectURL(blobOf(body));
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

export function attachMediaSource(element, pagesOrBody, mime) {
  releaseMediaSource(element);
  const held = createMediaUrl(pagesOrBody, mime);
  attached.set(element, held.revoke);
  element.setAttribute("src", held.url);
  watchRemoved(element.ownerDocument);
  return held.url;
}
