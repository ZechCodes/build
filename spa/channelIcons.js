import { readFileSync, readdirSync } from "node:fs";

const publicDir = new URL("./public/", import.meta.url);
const devDir = new URL("./channel-icons/", import.meta.url);
const devNames = {
  "favicon.svg": "favicon-dev.svg",
  "icon-192.png": "icon-dev-192.png",
  "icon-512.png": "icon-dev-512.png",
  "apple-touch-icon.png": "apple-touch-icon-dev.png",
  "manifest.webmanifest": "manifest-dev.webmanifest",
};

// Keep dev sources outside public/: a production build copies precisely the
// original public assets. Dev references use distinct URLs for browser caches.
export function channelIconsPlugin(channel) {
  if (channel !== "dev") return null;
  return {
    name: "build-channel-icons",
    apply: "build",
    transformIndexHtml: {
      order: "post",
      handler(html) {
        for (const [original, dev] of Object.entries(devNames)) {
          html = html.replaceAll(`/app/static/${original}`, `/app/static/${dev}`);
        }
        return html.replace('name="apple-mobile-web-app-title" content="Build"',
          'name="apple-mobile-web-app-title" content="Build Dev"');
      },
    },
    generateBundle() {
      for (const fileName of readdirSync(devDir).filter((file) => /\.(png|svg)$/.test(file))) {
        const source = readFileSync(new URL(fileName, devDir));
        this.emitFile({ type: "asset", fileName, source });
        // Retained URLs serve dev bytes too: installed PWAs and the push
        // worker can keep requesting them after the channel is deployed.
        const legacyName = Object.keys(devNames).find((name) => devNames[name] === fileName);
        if (legacyName) this.emitFile({ type: "asset", fileName: legacyName, source });
      }
      const manifest = JSON.parse(readFileSync(new URL("manifest.webmanifest", publicDir), "utf8"));
      manifest.name = manifest.short_name = "Build Dev";
      manifest.icons = manifest.icons.map((icon) => ({
        ...icon, src: icon.src.replace("icon-", "icon-dev-"),
      }));
      for (const fileName of ["manifest.webmanifest", devNames["manifest.webmanifest"]]) {
        this.emitFile({ type: "asset", fileName, source: JSON.stringify(manifest, null, 2) + "\n" });
      }
    },
  };
}
