const svg = (viewBox, paths, className = "") => `<svg class="${className}" viewBox="${viewBox}" aria-hidden="true" focusable="false">${paths}</svg>`;

const wifiIcon = svg("0 0 18 14", `
  <path d="M1.2 4.2a11.9 11.9 0 0 1 15.6 0"/>
  <path d="M4.1 7.5a7.6 7.6 0 0 1 9.8 0"/>
  <path d="M7.1 10.7a3.1 3.1 0 0 1 3.8 0"/>
  <circle cx="9" cy="12.6" r="1" class="solid"/>
`, "status-stroke");

const cellularIcon = svg("0 0 18 14", `
  <rect x="1" y="9" width="2.5" height="4" rx=".7"/>
  <rect x="5.5" y="6.5" width="2.5" height="6.5" rx=".7"/>
  <rect x="10" y="3.5" width="2.5" height="9.5" rx=".7"/>
  <rect x="14.5" y=".5" width="2.5" height="12.5" rx=".7"/>
`, "status-fill");

const batteryIcon = (percentage = 82) => svg("0 0 27 13", `
  <rect x=".75" y=".75" width="22" height="11.5" rx="3.2" class="battery-case"/>
  <rect x="2.4" y="2.4" width="${Math.max(3, 18.7 * percentage / 100)}" height="8.2" rx="1.8" class="battery-level"/>
  <path d="M24 4.2v4.6c1-.2 1.7-1.1 1.7-2.3S25 4.4 24 4.2Z" class="battery-cap"/>
`, "battery-icon");

const searchIcon = svg("0 0 16 16", `<circle cx="7" cy="7" r="4.4"/><path d="m10.4 10.4 4 4"/>`, "menu-stroke");
const controlCenterIcon = svg("0 0 18 16", `<path d="M2 4h14M2 12h14"/><circle cx="6" cy="4" r="2" class="solid-bg"/><circle cx="12" cy="12" r="2" class="solid-bg"/>`, "menu-stroke");
const appleIcon = svg("0 0 18 20", `<path class="apple-glyph" d="M14.7 10.6c0-2.6 2.1-3.8 2.2-3.9a4.8 4.8 0 0 0-3.8-2.1c-1.6-.2-3.1 1-3.9 1-.8 0-2-1-3.3-1-1.7 0-3.3 1-4.2 2.4-1.8 3.1-.5 7.7 1.3 10.2.9 1.2 1.9 2.6 3.2 2.5 1.3-.1 1.8-.8 3.4-.8 1.6 0 2 .8 3.4.8 1.4 0 2.3-1.3 3.1-2.5 1-1.4 1.4-2.8 1.4-2.9-.1 0-2.8-1.1-2.8-3.7ZM12 2.9A4.6 4.6 0 0 0 13.1 0a4.7 4.7 0 0 0-3 1.5A4.3 4.3 0 0 0 9 4.3c1.1.1 2.2-.5 3-1.4Z"/>`);

const dockIcon = (kind, label, content, extraClass = "") => `
  <div class="dock-item" aria-label="${label}">
    <div class="dock-app dock-${kind} ${extraClass}">${content}</div>
    ${kind === "build" || kind === "terminal" ? '<span class="dock-running"></span>' : ""}
  </div>`;

const finder = dockIcon("finder", "Finder", svg("0 0 56 56", `
  <path d="M28 2H12C6.5 2 2 6.5 2 12v32c0 5.5 4.5 10 10 10h16Z" class="finder-light"/>
  <path d="M28 2h16c5.5 0 10 4.5 10 10v32c0 5.5-4.5 10-10 10H28Z" class="finder-dark"/>
  <path d="M28 4c-2.3 6.2-3.2 12.6-2.8 19.4" class="finder-line"/>
  <path d="M12.5 22.8h.1M39 22.8h.1" class="finder-eye"/>
  <path d="M11.5 36.4c4.4 3.7 9.8 5.6 16.3 5.6 6.7 0 12.2-1.9 16.7-5.6" class="finder-line"/>
`));

const safari = dockIcon("safari", "Safari", svg("0 0 56 56", `
  <circle cx="28" cy="28" r="23" class="safari-face"/>
  <circle cx="28" cy="28" r="18.5" class="safari-ring"/>
  <path d="M28 10v3M28 43v3M10 28h3M43 28h3M15.3 15.3l2.2 2.2M38.5 38.5l2.2 2.2M40.7 15.3l-2.2 2.2M17.5 38.5l-2.2 2.2" class="safari-ticks"/>
  <path d="m34.8 20.8-4 10-9.6 4.4 4-10Z" class="safari-needle"/>
`));

const messages = dockIcon("messages", "Messages", svg("0 0 56 56", `
  <path d="M28 8c13.1 0 22 7.4 22 17.4S40.7 43 28.1 43c-2.8 0-5.2-.4-7.4-1.1L10 47l3.7-9.1C8.9 34.8 6 30.4 6 25.4 6 15.4 14.9 8 28 8Z" class="messages-bubble"/>
  <circle cx="20" cy="25.5" r="2"/><circle cx="28" cy="25.5" r="2"/><circle cx="36" cy="25.5" r="2"/>
`));

const terminal = dockIcon("terminal", "Terminal", svg("0 0 56 56", `
  <rect x="6" y="8" width="44" height="40" rx="8" class="terminal-face"/>
  <path d="m15 20 7 7-7 7M27 35h13" class="terminal-code"/>
`));

const notes = dockIcon("notes", "Notes", svg("0 0 56 56", `
  <rect x="7" y="5" width="42" height="46" rx="8" class="notes-paper"/>
  <path d="M7 16h42" class="notes-head"/>
  <path d="M15 25h26M15 32h22M15 39h25" class="notes-lines"/>
`));

const files = dockIcon("files", "Files", svg("0 0 56 56", `
  <path d="M6 15c0-4.4 3.6-8 8-8h11l4 5h13c4.4 0 8 3.6 8 8v24c0 3.3-2.7 6-6 6H12c-3.3 0-6-2.7-6-6Z" class="files-folder"/>
  <path d="M6 20h44" class="files-fold"/>
`));

const settings = dockIcon("settings", "Settings", svg("0 0 56 56", `
  <path d="m32 6 1.8 5.4c1.3.5 2.5 1.2 3.6 2.1l5.6-1.2 4 6.9-3.9 4.2c.1.8.2 1.5.2 2.4s-.1 1.7-.2 2.5l3.9 4.2-4 6.9-5.6-1.2c-1.1.9-2.3 1.6-3.6 2.1L32 46h-8l-1.8-5.7c-1.3-.5-2.5-1.2-3.6-2.1L13 39.4l-4-6.9 3.9-4.2a13 13 0 0 1 0-4.9L9 19.2l4-6.9 5.6 1.2c1.1-.9 2.3-1.6 3.6-2.1L24 6Z" class="settings-gear"/>
  <circle cx="28" cy="26" r="7" class="settings-center"/>
`));

const build = dockIcon("build", "Build", `<span class="dock-build-mark"><img src="./build-mark.svg" alt=""></span>`, "dock-build");

const trash = dockIcon("trash", "Trash", svg("0 0 56 56", `
  <path d="M15 16h27l-2.6 33H18Z" class="trash-bin"/><path d="M13 13h31M22 13l2-5h9l2 5M24 22v19M33 22v19" class="trash-lines"/>
`));

const divider = `<span class="dock-divider"></span>`;
const macDockContents = () => [finder, safari, messages, terminal, notes, settings, build, divider, trash].join("");
const ipadDockContents = () => [files, safari, messages, notes, settings, build].join("");

function statusRight({ cellular = false, percentage = 82 } = {}) {
  return `<span class="system-status-right">${cellular ? cellularIcon : ""}${wifiIcon}${batteryIcon(percentage)}</span>`;
}

function macbookShell(appHtml) {
  return `<div class="system-shell macbook-shell">
    <div class="desktop-wallpaper" aria-hidden="true"><i></i><i></i><i></i></div>
    <header class="mac-menu-bar">
      <div class="mac-menu-left">${appleIcon}<strong>Build</strong><span>File</span><span>Edit</span><span>View</span><span>Go</span><span>Window</span><span>Help</span></div>
      <div class="camera-safe-area" aria-hidden="true"></div>
      <div class="mac-menu-right">${batteryIcon(86)}${wifiIcon}${searchIcon}${controlCenterIcon}<span>Sat Sep 19</span><strong>9:41 AM</strong></div>
    </header>
    <section class="mac-window system-window">
      <div class="mac-titlebar"><span class="traffic-lights"><i></i><i></i><i></i></span><span class="window-title">Build — Launch</span></div>
      <div class="system-app-host">${appHtml}</div>
    </section>
    <nav class="system-dock mac-dock" aria-label="Dock">${macDockContents()}</nav>
  </div>`;
}

function ipadShell(appHtml) {
  return `<div class="system-shell ipad-shell">
    <div class="tablet-wallpaper" aria-hidden="true"><i></i><i></i></div>
    <header class="ipad-status-bar"><strong>9:41</strong>${statusRight({ percentage: 82 })}</header>
    <section class="ipad-window system-window">
      <div class="ipad-windowbar"><span class="ipad-window-controls"><i></i><i></i><i></i></span><span class="ipad-window-title">Build — Launch</span></div>
      <div class="system-app-host">${appHtml}</div>
    </section>
    <nav class="system-dock ipad-dock" aria-label="Dock">${ipadDockContents()}</nav>
    <div class="home-indicator" aria-hidden="true"></div>
  </div>`;
}

function iphoneShell(appHtml) {
  return `<div class="system-shell iphone-shell">
    <header class="iphone-status-bar"><strong>9:41</strong><span class="island-safe-area" aria-hidden="true"></span>${statusRight({ cellular: true, percentage: 82 })}</header>
    <div class="iphone-app-host">${appHtml}</div>
    <div class="iphone-home-safe"><div class="home-indicator" aria-hidden="true"></div></div>
  </div>`;
}

export function renderSystemShell(profile, appHtml) {
  if (profile === "macbook") return macbookShell(appHtml);
  if (profile === "ipad") return ipadShell(appHtml);
  if (profile === "iphone") return iphoneShell(appHtml);
  return appHtml;
}
