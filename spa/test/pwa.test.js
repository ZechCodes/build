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
    expect(metaContent("theme-color", "(prefers-color-scheme: light)")).toBe("#fafafa");
    expect(metaContent("theme-color", "(prefers-color-scheme: dark)")).toBe("#15171c");
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
      theme_color: "#fafafa",
      background_color: "#fafafa",
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
