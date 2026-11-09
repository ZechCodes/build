// Light / dark / system appearance.
//
// One attribute drives everything: styles.css declares its tokens twice —
// `:root` (light) and `:root[data-theme="dark"]` — and this module is the only
// thing that writes that attribute. Nothing else in the client knows a colour.
//
// "system" is the default and stays live: a preference to follow the OS is a
// standing instruction, not a one-time read, so installTheme keeps listening
// and repaints when the OS flips at dusk.
//
// Browser-scoped (localStorage), like every other preference this client keeps:
// the device picker, agent defaults, read state, the collapsed rail.

export const THEME_KEY = "build.theme";
export const THEME_CHOICES = ["light", "dark", "system"];

const THEME_LABEL = { light: "Light", dark: "Dark", system: "System" };

// The page background of each theme, mirrored from styles.css's --bg so the
// browser's own chrome (iOS status bar, Android toolbar) matches the page.
const CHROME_COLOR = { light: "#f3f7f8", dark: "#07151b" };

// Identity badge backgrounds and ink. All twelve tokens exist in both themes.
const BADGE_THEMES = {
  light: { ink: "#ffffff", hues: [
    "#a62b3e", "#a63e18", "#805b00", "#626b00", "#287139", "#00705d",
    "#006d80", "#265da3", "#554cb0", "#79449d", "#9b367b", "#94504e",
  ] },
  dark: { ink: "#07151b", hues: [
    "#f6a3b1", "#f3ae8d", "#e5c166", "#c6cf77", "#8fd4a0", "#7ad5bf",
    "#7fcddd", "#9bbdf5", "#b7aff3", "#d2a6eb", "#eba2d1", "#dcb0a6",
  ] },
};

function applyBadgeTokens(theme, doc) {
  const palette = BADGE_THEMES[theme];
  doc.documentElement.style.setProperty("--badge-ink", palette.ink);
  palette.hues.forEach((color, index) => doc.documentElement.style.setProperty(`--badge-${index + 1}`, color));
}

const normalize = (choice) => (THEME_CHOICES.includes(choice) ? choice : "system");

/** The stored preference, or "system". Never throws: an unreadable or unknown
 *  value means the user has expressed nothing, which is what "system" is. */
export function loadThemePreference(storage = localStorage) {
  try {
    return normalize(storage.getItem(THEME_KEY));
  } catch {
    return "system";
  }
}

/** Persist the preference. Returns what was stored so a caller can repaint from
 *  it without re-reading. */
export function saveThemePreference(choice, storage = localStorage) {
  const value = normalize(choice);
  try {
    storage.setItem(THEME_KEY, value);
  } catch {
    /* private mode: the session keeps working, the preference just does not stick */
  }
  return value;
}

/** Which of the two real themes a preference means right now. Pure — the OS
 *  answer arrives as a boolean, so this is testable without a browser. */
export function resolveTheme(preference, prefersDark) {
  const choice = normalize(preference);
  if (choice === "system") return prefersDark ? "dark" : "light";
  return choice;
}

/** The OS appearance query, or null where the browser cannot answer one. */
export function prefersDarkQuery() {
  try {
    return globalThis.matchMedia ? globalThis.matchMedia("(prefers-color-scheme: dark)") : null;
  } catch {
    return null;
  }
}

/** Paint a resolved theme: the attribute styles.css keys off, plus the browser
 *  chrome colour so the page does not sit under a mismatched status bar. */
export function applyTheme(theme, doc = document) {
  const resolved = theme === "dark" ? "dark" : "light";
  doc.documentElement.setAttribute("data-theme", resolved);
  applyBadgeTokens(resolved, doc);
  doc.querySelectorAll('meta[name="theme-color"]').forEach((meta) =>
    meta.setAttribute("content", CHROME_COLOR[resolved]),
  );
  return resolved;
}

/** Resolve the stored preference against the OS and paint it. Returns the
 *  theme now on screen. */
export function applyStoredTheme({ doc = document, storage = localStorage, media = prefersDarkQuery() } = {}) {
  return applyTheme(resolveTheme(loadThemePreference(storage), Boolean(media && media.matches)), doc);
}

/** Paint at boot, then keep following the OS for as long as the preference says
 *  "system". Returns a dispose() for tests and teardown. */
export function installTheme({ doc = document, storage = localStorage, media = prefersDarkQuery() } = {}) {
  applyStoredTheme({ doc, storage, media });
  if (!media || !media.addEventListener) return () => {};
  const onSystemChange = () => applyStoredTheme({ doc, storage, media });
  media.addEventListener("change", onSystemChange);
  return () => media.removeEventListener("change", onSystemChange);
}

/** The colours a ghostty terminal is built with, read from the page's own
 *  tokens so the pane's chrome and its canvas agree. A terminal is dark in both
 *  themes; the fallbacks are the light theme's values, for a mount that beats
 *  the stylesheet or a test with no CSS at all. */
export function terminalTheme(doc = document) {
  const tokens = doc.defaultView ? doc.defaultView.getComputedStyle(doc.documentElement) : null;
  const token = (name, fallback) => (tokens ? tokens.getPropertyValue(name).trim() : "") || fallback;
  return { background: token("--term-bg", "#15161e"), foreground: token("--term-fg", "#a9b1d6") };
}

/** The Account page's three-segment switch. Pure HTML, in the sheet's own
 *  .segmented idiom — the chosen segment carries .primary, like every other
 *  "this one is the answer" control in the client. */
export function themeControlHtml(preference) {
  const current = normalize(preference);
  return `<div class="segmented" id="themepick" role="group" aria-label="Appearance">${THEME_CHOICES.map(
    (choice) =>
      `<button type="button" class="btn seg${choice === current ? " primary" : ""}" data-theme-choice="${choice}" aria-pressed="${choice === current}">${THEME_LABEL[choice]}</button>`,
  ).join("")}</div>`;
}

/** Wire a rendered themeControlHtml: a click stores the choice, repaints the
 *  page, and re-marks the chosen segment. */
export function bindThemeControl(host, { doc = document, storage = localStorage, media = prefersDarkQuery() } = {}) {
  host.querySelectorAll("[data-theme-choice]").forEach((button) => {
    button.onclick = () => {
      const chosen = saveThemePreference(button.dataset.themeChoice, storage);
      applyTheme(resolveTheme(chosen, Boolean(media && media.matches)), doc);
      host.querySelectorAll("[data-theme-choice]").forEach((other) => {
        const selected = other.dataset.themeChoice === chosen;
        other.classList.toggle("primary", selected);
        other.setAttribute("aria-pressed", String(selected));
      });
    };
  });
}
