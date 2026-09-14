// The conversation header identifies the harness that owns the selected agent.
// Harness artwork is trusted, bundled source; unknown provider ids receive the
// app's neutral agent mark rather than borrowing a known provider's identity.

import codexIcon from "../assets/harnesses/codex.svg?raw";
import claudeIcon from "../assets/harnesses/claude.svg?raw";
import opencodeIcon from "../assets/harnesses/opencode.svg?raw";
import opencodeDarkIcon from "../assets/harnesses/opencode-dark.svg?raw";
import piIcon from "../assets/harnesses/pi.svg?raw";
import { ICON_CIRCLE_DOT } from "./icons.js";

const ICONS_BY_PROVIDER = new Map([
  ["claude", claudeIcon],
  ["claude_adk", claudeIcon],
  ["codex", codexIcon],
  ["codex_app_server", codexIcon],
  ["opencode", opencodeIcon],
  ["pi", piIcon],
]);

/** Bundled icon markup for a wire provider id, neutral when it is unknown. */
export function harnessIcon(providerId) {
  return ICONS_BY_PROVIDER.get(providerId) || ICON_CIRCLE_DOT;
}

const openCodeIconHtml = () =>
  `<span class="rail-harness-icon-variant rail-harness-icon-light">${opencodeIcon}</span>`
  + `<span class="rail-harness-icon-variant rail-harness-icon-dark">${opencodeDarkIcon}</span>`;

/** The complete decorative header mark, kept outside the user-selected title. */
export function harnessIconHtml(providerId) {
  const knownProvider = ICONS_BY_PROVIDER.has(providerId);
  const icon = providerId === "opencode" ? openCodeIconHtml() : harnessIcon(providerId);
  return `<span class="rail-harness-icon" data-harness-icon="${knownProvider ? providerId : "unknown"}" aria-hidden="true">${icon}</span>`;
}
