// Where to get the bridge. One renderer, two hosts: the first-run gate (the
// account has no device yet) and Settings → Downloads (another machine, or an
// update). Every fact in it — the one-liner, the four builds, the checksums —
// comes from the api's /app/downloads payload; this module knows no URLs.
//
// Pure html first, then mount, the same shape as core/defaultHarness.js: the
// host ships the placeholder in its own markup and fills it once the api
// answers, so a slow or refusing api never leaves a blank screen behind.

import { esc } from "./text.js";

const UNAVAILABLE = "Downloads aren't available right now.";

const dim = (text, extra = "") => `<div class="dim" style="font-size:12.5px;${extra}">${text}</div>`;

const platformLink = (platform) => `<a href="${esc(platform.url)}" download>${esc(platform.label)}</a>`;

const primaryHtml = (platform) =>
  `<a class="btn primary" href="${esc(platform.url)}" download>Download for ${esc(platform.label)}</a>`;

const everyPlatformHtml = (platforms) =>
  `${dim("Pick the machine your code lives on.", "margin-bottom:6px")}
      <div style="display:flex;gap:14px;flex-wrap:wrap;font-size:13px">${platforms.map(platformLink).join("")}</div>`;

const installLineHtml = (command) =>
  `${dim("or install with one line", "margin:14px 0 6px")}
      <div style="display:flex;gap:8px;align-items:flex-start">
        <code class="mono" id="installcmd" style="flex:1;font-size:11px;line-height:1.5;padding:8px 10px;overflow-x:auto;white-space:pre-wrap;word-break:break-all">${esc(command)}</code>
        <button class="btn mini" id="copycmd">Copy</button></div>`;

const otherPlatformsHtml = (others, { checksums_url, releases_url }) => {
  const links = [
    ...others.map(platformLink),
    checksums_url ? `<a href="${esc(checksums_url)}">checksums</a>` : "",
    releases_url ? `<a href="${esc(releases_url)}">all releases</a>` : "",
  ].filter(Boolean);
  return links.length ? dim(`Other platforms: ${links.join(" · ")}`, "margin-top:12px") : "";
};

/** The block, painted from the api's payload. `platformKey` is looked up in the
 *  payload rather than branched on: this module knows the four keys only as
 *  strings the api also uses. */
export function downloadsHtml(downloads, platformKey) {
  const platforms = Array.isArray(downloads?.platforms) ? downloads.platforms : [];
  const matched = platforms.find((platform) => platform.key === platformKey);
  return `<div id="downloads">
      ${matched ? primaryHtml(matched) : everyPlatformHtml(platforms)}
      ${installLineHtml(downloads?.install_command ?? "")}
      ${otherPlatformsHtml(matched ? platforms.filter((platform) => platform !== matched) : [], downloads ?? {})}
      ${dim("Once paired, the bridge runs on your machine and holds its own key; Build's servers move ciphertext.", "margin-top:12px")}
    </div>`;
}

/** What a host embeds before mounting: the slot the payload lands in, and the
 *  one line a refusal gets to say. */
export function downloadsPlaceholderHtml() {
  return `<div id="downloads">${dim("loading…")}</div>
      <div class="adderr" id="downloadserr"></div>`;
}

/** Copy `text`, and say so for a moment. The gate and the downloads block share
 *  this so "Copied" means the same thing everywhere. */
export function bindCopyButton(button, text, clipboard) {
  if (!button) return;
  button.onclick = () => {
    clipboard?.writeText?.(text);
    button.textContent = "Copied";
    setTimeout(() => (button.textContent = "Copy"), 1500);
  };
}

/** Fill the host's placeholder from the api. A refusal names itself in
 *  #downloadserr and leaves every other control on the page usable — on the
 *  first-run screen the pairing code is what actually pairs a device, and it
 *  must survive a downloads route that is missing or closed. */
export async function mountDownloads(host, { fetchDownloads, platformKey, clipboard } = {}) {
  const slot = host?.querySelector?.("#downloads");
  if (!slot) return;
  let downloads;
  try {
    downloads = await fetchDownloads();
  } catch (failure) {
    slot.innerHTML = dim(UNAVAILABLE);
    const error = host.querySelector("#downloadserr");
    if (error) error.textContent = failure.message;
    return;
  }
  slot.outerHTML = downloadsHtml(downloads, platformKey);
  bindCopyButton(host.querySelector("#copycmd"), downloads?.install_command ?? "", clipboard);
}
