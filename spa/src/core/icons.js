// The app's icon set: lucide (ISC), imported as raw SVG at build time. No
// runtime icon library, no icon font, nothing fetched — each icon is a string
// the bundler inlines, so only the icons named here ship.
//
// Views import from HERE, never from lucide-static, so swapping the pack or
// resizing the set is one file's work.
//
// Every icon carries stroke="currentColor", so it takes the colour of whatever
// it sits in; size it from CSS. Each is a build-time constant and never user
// data — which is what lets the tab shell inline one as markup.

export { default as ICON_INBOX } from "lucide-static/icons/inbox.svg?raw";
export { default as ICON_CIRCLE_DOT } from "lucide-static/icons/circle-dot.svg?raw";
export { default as ICON_ELLIPSIS } from "lucide-static/icons/ellipsis.svg?raw";
export { default as ICON_PAPERCLIP } from "lucide-static/icons/paperclip.svg?raw";
export { default as ICON_X } from "lucide-static/icons/x.svg?raw";
export { default as ICON_ARROW_UP } from "lucide-static/icons/arrow-up.svg?raw";
