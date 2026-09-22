import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const styles = readFileSync(fileURLToPath(new URL("../src/styles.css", import.meta.url)), "utf8");
const themeBlocks = {
  light: styles.match(/:root \{([^}]+)\}/)?.[1],
  dark: styles.match(/:root\[data-theme="dark"\] \{([^}]+)\}/)?.[1],
};
const tokensIn = (block) => Object.fromEntries(
  [...block.matchAll(/--([\w-]+):\s*(#[0-9a-f]{6})/g)].map((match) => [match[1], match[2]]),
);

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
  it.each(["light", "dark"])("keeps every user bubble ink at WCAG AA contrast in %s", (theme) => {
    const tokens = tokensIn(themeBlocks[theme]);
    const background = tokens["user-bubble"];
    expect(background).toBeTruthy();
    for (const name of ["user-bubble-ink", "user-bubble-link", "user-bubble-muted", "user-bubble-warn", "user-bubble-error"]) {
      const foreground = tokens[name];
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
