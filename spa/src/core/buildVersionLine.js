// Which build this tab is running, where somebody looking for it will look.
//
// The bundle is stamped with `VITE_BUILD_VERSION` — the git sha CI built it at
// — and core/version.js already reads it to decide whether this tab has fallen
// behind the server. But it was never shown: the only way to learn which build
// was on screen was to read the bundle, and on the night it mattered there was
// no way at all. So it sits under Diagnostics, which is where somebody with a
// question about this tab is already standing.
//
// Short on the page and whole in the title and on the clipboard: seven
// characters is what a person compares at a glance, and forty is what a `git
// show` wants.

import { esc } from "./text.js";
import { copyText } from "./connectionDiagnosticsPanel.js";

/// How much of a sha a person reads. Git's own abbreviation, and enough to name
/// one build among the handful anybody is ever choosing between.
const SHORT_SHA_LENGTH = 7;

/// A version with nothing behind it: a dev server, or a bundle built without
/// the stamp. Named rather than blank, because "Build dev" answers the question
/// and an empty line does not.
const DEV = "dev";

/// Whether this version is a sha, and so has a short form worth showing. A tag,
/// a branch name or `dev` is shown whole — abbreviating those would cut words.
const looksLikeSha = (version) => /^[0-9a-f]{7,40}$/i.test(version);

/** What the line shows: the first seven characters of a sha, or the version as
 *  it was stamped for anything that is not one. */
export function shortBuildVersion(version) {
  const stamped = String(version || "").trim();
  if (!stamped) return DEV;
  return looksLikeSha(stamped) ? stamped.slice(0, SHORT_SHA_LENGTH) : stamped;
}

/** The whole version, for the title and the clipboard — what somebody pastes
 *  into a `git show` or into a report. */
export const fullBuildVersion = (version) => String(version || "").trim() || DEV;

/// The line itself. Not a panel: it is one fact, and a heading over one fact
/// costs more room than the fact.
export function buildVersionLineHtml(version) {
  const whole = fullBuildVersion(version);
  return `<div class="buildversion" id="buildversion">
      <span class="dim">Build</span>
      <span class="mono" id="buildversionsha" title="${esc(whole)}">${esc(shortBuildVersion(version))}</span>
      <button class="btn mini" id="buildversioncopy">Copy</button>
    </div>`;
}

/**
 * Wire the Copy button. The whole sha goes, never the short form — the short
 * form is for reading and the whole one is for pasting somewhere that has to
 * resolve it.
 *
 * The clipboard route is the Diagnostics panel's `copyText`, so this line and
 * the dump above it behave the same on a browser that refuses
 * `navigator.clipboard`: there is one answer to "how does this app copy
 * things", and a phone over plain http is exactly where both are read.
 */
export function mountBuildVersionLine(host, { version, clipboard = globalThis.navigator?.clipboard } = {}) {
  const button = host?.querySelector?.("#buildversioncopy");
  if (!button) return;
  button.onclick = async () => {
    const done = await copyText(fullBuildVersion(version), { clipboard });
    button.textContent = done ? "Copied" : "Press and hold to copy";
    setTimeout(() => (button.textContent = "Copy"), 1500);
  };
}
