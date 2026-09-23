import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const indexSource = readFileSync(
  fileURLToPath(new URL("../index.html", import.meta.url)),
  "utf8",
);
const manifest = JSON.parse(
  readFileSync(
    fileURLToPath(new URL("../public/manifest.webmanifest", import.meta.url)),
    "utf8",
  ),
);
// The frame is split across two sheets: the page lock in styles.css, the
// three-panel shell (and its scroll containers) in styles/shell.css.
const stylesSource =
  readFileSync(fileURLToPath(new URL("../src/styles.css", import.meta.url)), "utf8") +
  readFileSync(fileURLToPath(new URL("../src/styles/shell.css", import.meta.url)), "utf8");
function metaContent(name, media) {
  const tags = indexSource.match(/<meta\s+[^>]+>/g) ?? [];
  const tag = tags.find((candidate) =>
    candidate.includes(`name="${name}"`) &&
    (media === undefined || candidate.includes(`media="${media}"`)),
  );
  return tag?.match(/content="([^"]+)"/)?.[1];
}

describe("iPad PWA document metadata", () => {
  it("opts into standalone safe areas without losing keyboard viewport behavior", () => {
    const viewport = metaContent("viewport");
    expect(viewport).toContain("width=device-width");
    expect(viewport).toContain("viewport-fit=cover");
    expect(viewport).toContain("interactive-widget=resizes-content");
    expect(metaContent("apple-mobile-web-app-capable")).toBe("yes");
  });

  it("provides theme-aware chrome and iOS install assets", () => {
    expect(metaContent("theme-color", "(prefers-color-scheme: light)")).toBe("#f3f7f8");
    expect(metaContent("theme-color", "(prefers-color-scheme: dark)")).toBe("#07151b");
    expect(indexSource).toMatch(/<link\s+rel="manifest"\s+href="[^"]+"\s*\/>/);
    expect(indexSource).toMatch(/<link\s+rel="apple-touch-icon"\s+href="[^"]+"\s*\/>/);
  });
});

describe("installable PWA manifest", () => {
  it("keeps authentication inside the standalone app", () => {
    expect(manifest).toMatchObject({
      display: "standalone",
      display_override: ["standalone"],
      scope: "/",
      start_url: "/app/",
      id: "/app/",
      lang: "en",
      dir: "ltr",
      orientation: "any",
      theme_color: "#f3f7f8",
      background_color: "#f3f7f8",
    });
  });

  it("includes icons at both required install sizes", () => {
    expect(manifest.icons).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ sizes: "192x192", type: "image/png" }),
        expect.objectContaining({ sizes: "512x512", type: "image/png" }),
      ]),
    );
  });
});

describe("standalone shell styles", () => {
  it("suppresses page overscroll and clears more than just the terminal key bar", () => {
    expect(stylesSource).toContain("overscroll-behavior");
    expect(stylesSource.match(/env\(safe-area-inset-bottom/g)?.length ?? 0).toBeGreaterThanOrEqual(2);
  });

  // The standalone regression this pins: the page itself scrolled alongside the
  // rail and the content column, because the frame relied on body scroll with a
  // sticky 100vh sidebar (100vh overflows the standalone viewport). The frame
  // is now viewport-locked: the page never scrolls, each column scrolls itself.
  it("locks the app frame to the viewport instead of scrolling the page", () => {
    const bodyRule = stylesSource.match(/(^|\n)body \{[^}]+\}/)?.[0] ?? "";
    expect(bodyRule).toContain("height:100dvh");
    expect(bodyRule).toContain("flex-direction:column");
    const shellRule = stylesSource.match(/\n#shell \{[^}]+\}/)?.[0] ?? "";
    expect(shellRule).toContain("overflow:clip");
    expect(shellRule).toContain("min-height:0");
  });

  it("scrolls the content column and the rail independently", () => {
    const mainRule = stylesSource.match(/\n#shell main#root:not\(\.surface\) \{[^}]+\}/)?.[0] ?? "";
    expect(mainRule).toContain("overflow-y:auto");
    const railRule = stylesSource.match(/\n#inbox-rail \{[^}]+\}/)?.[0] ?? "";
    expect(railRule).not.toContain("position:sticky");
    expect(railRule).not.toContain("100vh");
    expect(stylesSource).toContain("#inbox-list { flex:1 1 auto; min-height:0; overflow-y:auto;");
  });
});
