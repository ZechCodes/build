import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const styles = readFileSync(fileURLToPath(new URL("../src/styles.css", import.meta.url)), "utf8");
const themeBlock = (selector) => styles.match(new RegExp(`${selector} \\{([^}]+)\\}`))[1];
const token = (block, name) => block.match(new RegExp(`--${name}:\\s*(#[0-9a-f]{6})`))?.[1];

function luminance(hex) {
  const channels = hex.slice(1).match(/../g).map((part) => Number.parseInt(part, 16) / 255);
  const [red, green, blue] = channels.map((value) =>
    value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4);
  return red * 0.2126 + green * 0.7152 + blue * 0.0722;
}

function contrast(one, two) {
  const [lighter, darker] = [luminance(one), luminance(two)].sort((a, b) => b - a);
  return (lighter + 0.05) / (darker + 0.05);
}

describe("chat bubble colours", () => {
  it.each([":root", ':root\\[data-theme="dark"\\]'])("keeps every user bubble ink at WCAG AA contrast in %s", (selector) => {
    const block = themeBlock(selector);
    const background = token(block, "user-bubble");
    expect(background).toBeTruthy();
    for (const name of ["user-bubble-ink", "user-bubble-link", "user-bubble-muted", "user-bubble-warn", "user-bubble-error"]) {
      const foreground = token(block, name);
      expect(foreground, name).toBeTruthy();
      expect(contrast(background, foreground), name).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("gives reader messages green and other agents the former neutral surface", () => {
    expect(styles).toMatch(/\.thread-message:is\(\.user, \.agent, \.from-agent\) \.thread-comment-card[^}]+background:var\(--panel2\)/);
    expect(styles).toMatch(/\.thread-message\.user \.thread-comment-card \{[^}]+background:var\(--user-bubble\)/);
    expect(styles).toMatch(/\.thread-message\.user \.thread-body \{ color:var\(--user-bubble-ink\)/);
    expect(styles).toMatch(/\.thread-message:is\(\.agent, \.from-agent\) \{ align-self:flex-start; \}/);
    for (const property of ["body", "link", "muted", "warn", "error"]) {
      const selector = property === "body" ? "thread-body" : property === "link" ? "thread-revision-link" :
        property === "warn" ? "delivery-status.uncertain" : property === "error" ? "delivery-status.failed" : "thread-message-footer";
      expect(styles).toContain(selector);
    }
    expect(styles).not.toMatch(/\.thread-message\.from-agent \.thread-comment-card \{ background:var\(--accent-soft\)/);
  });
});
