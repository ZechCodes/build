// @ts-check
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { defineConfig } from "astro/config";

const LAB_DIR = fileURLToPath(new URL("./src/lab/", import.meta.url));
const HERO_DIR = fileURLToPath(new URL("./src/hero/", import.meta.url));
const LAB_COPY = "\0lab-copy:";

// The notifications lab (src/pages/lab/, #311) runs the hero's own modules.
// Shared with the home page, the build would split them into a chunk both
// load, and the home page's script would wait on a second request before
// the hero could start. So the lab bundles its own copy of every hero
// module it reaches, from the same source files, and the home page's
// script stays one file as it was.
function labOwnCopy() {
  return {
    name: "build-lab-own-copy",
    apply: "build",
    enforce: "pre",
    async resolveId(source, importer, options) {
      if (!importer) return null;
      const copied = importer.startsWith(LAB_COPY);
      if (!copied && !importer.startsWith(LAB_DIR)) return null;
      const from = copied ? importer.slice(LAB_COPY.length) : importer;
      const resolved = await this.resolve(source, from, { ...options, skipSelf: true });
      if (!resolved || resolved.external || !resolved.id.startsWith(HERO_DIR)) return resolved;
      return `${LAB_COPY}${resolved.id}`;
    },
    load(id) {
      return id.startsWith(LAB_COPY) ? readFile(id.slice(LAB_COPY.length), "utf8") : null;
    },
  };
}

// The page is a static document served by skriftapp from
// skriftapp/buildapp/landing/generated/, and reached at getbuild.ing/ — its
// assets are reached one directory deeper, at /landing/generated/_astro/*,
// which is what assetsPrefix stamps into the emitted HTML.
//
// The app's CSP is `script-src 'self' 'wasm-unsafe-eval'; style-src 'self'
// 'unsafe-inline'` with no CDN, so every script must be an external
// same-origin file: `inlineStylesheets: "never"` and an inline limit of zero
// keep Astro and Vite from folding small bundles into the document, where the
// CSP would drop them.
export default defineConfig({
  output: "static",
  outDir: "../skriftapp/buildapp/landing/generated",
  build: {
    assetsPrefix: "/landing/generated",
    inlineStylesheets: "never",
  },
  vite: {
    plugins: [labOwnCopy()],
    build: {
      assetsInlineLimit: 0,
    },
  },
});
