// The app's icon set: lucide (ISC), imported as raw SVG at build time. No
// runtime icon library, no icon font, nothing fetched — each icon is a string
// the bundler inlines, so only the icons named here ship.
//
// Views import from HERE, never from lucide-static, so swapping the pack or
// resizing the set is one file's work.
//
// Every icon carries stroke="currentColor", so it takes the colour of whatever
// it sits in. Each is a build-time constant and never user data — which is what
// lets the tab shell inline one as markup.
//
// SIZE IT FROM CSS, at every call site. The pack's SVGs carry an intrinsic
// 24x24, which is twice the font of a mini button — an unsized icon does not
// merely look large, it makes its button taller than everything beside it. Each
// place that renders one states its size once, near the rule that styles the
// control it sits in.

export { default as ICON_INBOX } from "lucide-static/icons/inbox.svg?raw";
export { default as ICON_CIRCLE_DOT } from "lucide-static/icons/circle-dot.svg?raw";
export { default as ICON_ELLIPSIS } from "lucide-static/icons/ellipsis.svg?raw";
export { default as ICON_PAPERCLIP } from "lucide-static/icons/paperclip.svg?raw";
export { default as ICON_X } from "lucide-static/icons/x.svg?raw";
export { default as ICON_ARROW_RIGHT } from "lucide-static/icons/arrow-right.svg?raw";
export { default as ICON_SQUARE } from "lucide-static/icons/square.svg?raw";
export { default as ICON_FOLDERS } from "lucide-static/icons/folders.svg?raw";
export { default as ICON_PLUS } from "lucide-static/icons/plus.svg?raw";
export { default as ICON_PIN } from "lucide-static/icons/pin.svg?raw";
export { default as ICON_CHEVRON_DOWN } from "lucide-static/icons/chevron-down.svg?raw";
export { default as ICON_SETTINGS } from "lucide-static/icons/settings.svg?raw";
export { default as ICON_WIFI_OFF } from "lucide-static/icons/wifi-off.svg?raw";
export { default as ICON_CHEVRON_RIGHT } from "lucide-static/icons/chevron-right.svg?raw";
export { default as ICON_MESSAGE_SQUARE } from "lucide-static/icons/message-square.svg?raw";
export { default as ICON_EXTERNAL_LINK } from "lucide-static/icons/external-link.svg?raw";
export { default as ICON_CHECK } from "lucide-static/icons/check.svg?raw";
export { default as ICON_REFRESH } from "lucide-static/icons/refresh-cw.svg?raw";
export { default as ICON_HISTORY } from "lucide-static/icons/history.svg?raw";
export { default as ICON_GIT_MERGE } from "lucide-static/icons/git-merge.svg?raw";
export { default as ICON_FILE } from "lucide-static/icons/file.svg?raw";
export { default as ICON_GIT_BRANCH } from "lucide-static/icons/git-branch.svg?raw";
export { default as ICON_GIT_GRAPH } from "lucide-static/icons/git-graph.svg?raw";
export { default as ICON_FOLDER } from "lucide-static/icons/folder.svg?raw";
export { default as ICON_FILE_TEXT } from "lucide-static/icons/file-text.svg?raw";
export { default as ICON_FILE_CODE } from "lucide-static/icons/file-code.svg?raw";
export { default as ICON_FILE_IMAGE } from "lucide-static/icons/file-image.svg?raw";
export { default as ICON_FILE_ARCHIVE } from "lucide-static/icons/file-archive.svg?raw";
export { default as ICON_FILE_AUDIO } from "lucide-static/icons/file-audio.svg?raw";
export { default as ICON_FILE_VIDEO } from "lucide-static/icons/file-video.svg?raw";
export { default as ICON_FILE_SPREADSHEET } from "lucide-static/icons/file-spreadsheet.svg?raw";
