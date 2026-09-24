// The window every DOM suite runs in is focused, as the tab a user is looking
// at is. jsdom answers `document.hasFocus()` false until an element has been
// focused, and the client's automatic read marks wait for a focused window
// (src/core/readerPresence.js), so without this every suite would be a window
// nobody is at. A suite about that case spies on `hasFocus` itself.

if (typeof globalThis.Document === "function") {
  globalThis.Document.prototype.hasFocus = function hasFocus() {
    return true;
  };
}
