// Client-side, render-time secret masking for dotenv files. This is a
// screen-privacy feature (screen-shares / screenshots): a secret-like value in
// a `.env` file — whether shown in the Files preview or inside a diff — renders
// as a fixed dot block until the viewer clicks to reveal it. Nothing here leaves
// the browser; the bridge is untouched.
//
// Two masking triggers combine (either one masks):
//   1. shape-based  — the value looks like a token/key/hash (isSecretLikeValue)
//   2. name-based   — the KEY name reads like a secret (isSecretLikeKeyName),
//                     masking ANY non-empty value (short passwords, symbols…)
//
// SECURITY: the Files preview keeps the real value OUT of the DOM entirely
// (renderDotenvSourceHtml returns dots + a JS-side `secrets` array); the diff
// path is a pure, deterministic string renderer (so poll repaints never flicker)
// and carries the escaped value in a data attribute, swapped in on click. Every
// value flows through esc() before it can reach any HTML string.

import { esc } from "./text.js";

/** The fixed reveal placeholder: always 10 dots, so the block never leaks the
 *  secret's real length. */
export const SPOILER_DOTS = "●".repeat(10);

/** True for a dotenv-format file by basename: `.env`, `.env.*`, or `*.envrc`. */
export function isDotenvPath(path) {
  const basename = String(path ?? "").split("/").pop();
  if (!basename) return false;
  return basename === ".env" || basename.startsWith(".env.") || basename.endsWith(".envrc");
}

// The secret-word key-name segments (already uppercased). Segment match on `_`,
// never substring: PRIVATE_KEY / DB_PASS mask; KEYCLOAK_HOST / COMPASS_URL do not.
const SECRET_KEY_SEGMENTS = new Set([
  "KEY",
  "TOKEN",
  "SECRET",
  "PASSWORD",
  "PASSWD",
  "PASS",
  "PWD",
  "APIKEY",
  "CREDENTIAL",
  "CREDENTIALS",
  "PASSPHRASE",
]);

// The secret-value shape charset. `/` is deliberately excluded so filesystem
// paths do not false-positive; `:` is absent too, so URLs stay visible.
const SECRET_VALUE_CHARSET = /^[A-Za-z0-9._+=-]+$/;

/** Shape rule: an (already unquoted) value looks secret when it is at least 20
 *  chars, is composed entirely of the secret charset, and holds a digit. This
 *  catches JWTs, hex keys, sk-/ghp_ keys, and base64/base64url — while leaving
 *  hostnames (no digit), URLs (colon/slash), and word chains visible. */
export function isSecretLikeValue(value) {
  const text = String(value ?? "");
  return text.length >= 20 && SECRET_VALUE_CHARSET.test(text) && /[0-9]/.test(text);
}

/** Name rule: the KEY name reads like a secret when any `_`-separated segment
 *  (case-insensitive) is one of the secret words. */
export function isSecretLikeKeyName(key) {
  return String(key ?? "")
    .toUpperCase()
    .split("_")
    .some((segment) => SECRET_KEY_SEGMENTS.has(segment));
}

// A dotenv assignment: leading whitespace, an optional `export `, the KEY, then
// `=`. Group 1 is everything through `=` (the reconstructable prefix), group 2
// is the bare key, group 3 is the raw remainder. Comment lines and lines with no
// `=` simply do not match.
const DOTENV_ASSIGNMENT = /^(\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=)(.*)$/;

/** Parse one dotenv line and decide whether its value is secret. Returns
 *  `{ masked: false }` for a non-assignment / comment / empty-value line, or
 *  `{ masked: true, prefix, value, suffix }` where `prefix + value + suffix`
 *  reconstructs the line exactly. `prefix` runs through `=` plus any opening
 *  quote; `value` is the bare secret span; `suffix` is any closing quote plus a
 *  trailing inline comment / whitespace. */
export function maskDotenvLine(line) {
  const match = DOTENV_ASSIGNMENT.exec(String(line ?? ""));
  if (!match) return { masked: false };
  const [, throughEquals, key, rest] = match;
  const leadWhitespace = rest.match(/^\s*/)[0];
  const afterLead = rest.slice(leadWhitespace.length);

  let openQuote = "";
  let closeQuote = "";
  let value;
  let trailing;
  const quote = afterLead[0];
  if (quote === '"' || quote === "'") {
    const close = afterLead.indexOf(quote, 1);
    if (close !== -1) {
      openQuote = quote;
      value = afterLead.slice(1, close);
      closeQuote = quote;
      trailing = afterLead.slice(close + 1);
    }
  }
  if (value === undefined) {
    // Unquoted: the value is the leading run of non-whitespace; anything after
    // (a ` # comment` or trailing whitespace) is not part of the value.
    const unquoted = /^(\S*)([\s\S]*)$/.exec(afterLead);
    value = unquoted[1];
    trailing = unquoted[2];
  }

  const isSecret = value.length > 0 && (isSecretLikeValue(value) || isSecretLikeKeyName(key));
  if (!isSecret) return { masked: false };
  return {
    masked: true,
    prefix: throughEquals + leadWhitespace + openQuote,
    value,
    suffix: closeQuote + trailing,
  };
}

/** The diff-path spoiler span: fixed dots on screen, the escaped real value in
 *  `data-secret` for click-to-reveal. Pure and deterministic (safe for the
 *  diff's poll-repaint freeze contract); revealed state resets on repaint. */
export function spoilerSpanHtml(value) {
  return `<span class="spoiler" data-secret="${esc(value)}" title="Click to reveal/hide secret">${SPOILER_DOTS}</span>`;
}

/** The diff cell HTML for one dotenv line: a masked line becomes
 *  `esc(prefix) + spoiler + esc(suffix)`; a non-secret line returns null so the
 *  caller falls back to its normal (syntax-highlighted) rendering. */
export function maskedDiffCellHtml(line) {
  const seg = maskDotenvLine(line);
  if (!seg.masked) return null;
  return esc(seg.prefix) + spoilerSpanHtml(seg.value) + esc(seg.suffix);
}

/** The Files-preview source view for a dotenv file. Returns the `<pre>` body
 *  HTML plus a `secrets` array: masked values live ONLY in that array (indexed
 *  by `data-secret-index`), never in the returned HTML, so the initial DOM has
 *  nothing to copy or inspect. Reveal is wired by the view against the array. */
export function renderDotenvSourceHtml(text) {
  const secrets = [];
  const body = String(text ?? "")
    .split("\n")
    .map((line) => {
      const seg = maskDotenvLine(line);
      if (!seg.masked) return esc(line);
      const index = secrets.push(seg.value) - 1;
      return `${esc(seg.prefix)}<span class="spoiler" data-secret-index="${index}" title="Click to reveal/hide secret">${SPOILER_DOTS}</span>${esc(seg.suffix)}`;
    })
    .join("\n");
  return { html: `<pre class="fsrc dotenv"><code>${body}</code></pre>`, secrets };
}

/** Diff-path click helper: if `target` is a `data-secret` spoiler, toggle it
 *  between dots and the revealed value (textContent swap — never innerHTML, so
 *  the escaped attribute value cannot re-inject markup) and return true. */
export function toggleSecretSpoiler(target) {
  const spoiler = target && target.closest ? target.closest(".spoiler[data-secret]") : null;
  if (!spoiler) return false;
  const revealed = spoiler.classList.toggle("on");
  spoiler.textContent = revealed ? spoiler.dataset.secret : SPOILER_DOTS;
  return true;
}
