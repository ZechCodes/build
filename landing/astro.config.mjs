// @ts-check
import { defineConfig } from "astro/config";

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
    build: {
      assetsInlineLimit: 0,
    },
  },
});
