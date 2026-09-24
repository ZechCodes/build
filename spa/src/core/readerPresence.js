// Whether the user is at this window, for the read marks nobody pressed.
//
// A page marks what it shows as read when content arrives or scrolls into
// view, not only when the user acts. Such a mark is also the user being here:
// the bridge's user session counts it, and "Done since you left" starts from
// where that session began. A desktop left showing a chat overnight is visible
// but nobody is reading it, so an automatic mark waits for the document to be
// both visible and focused, and whatever is on screen is marked when the user
// comes back. Explicit acts (sending, moving, opening) mark as they always did.

/** Visible and focused: someone is looking at this window. */
export function readerIsHere(doc = globalThis.document) {
  if (!doc) return false;
  return !doc.hidden && doc.hasFocus();
}

/** Call `back` whenever the user returns to this window: it is focused again,
 *  or its tab is shown while focused. Returns the unsubscribe. */
export function onReaderReturns(back) {
  const doc = globalThis.document;
  const win = globalThis.window;
  const returned = () => { if (readerIsHere(doc)) back(); };
  win?.addEventListener("focus", returned);
  doc?.addEventListener("visibilitychange", returned);
  return () => {
    win?.removeEventListener("focus", returned);
    doc?.removeEventListener("visibilitychange", returned);
  };
}
