import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

// libsodium-wrappers' ESM build imports "./libsodium.mjs", which actually lives
// in its sibling `libsodium` package — point the bundler at the real file.
const libsodiumEsm = fileURLToPath(
  new URL("./node_modules/libsodium/dist/modules-esm/libsodium.mjs", import.meta.url),
);
const libsodiumShim = {
  name: "libsodium-esm-shim",
  resolveId(source, importer) {
    if (source === "./libsodium.mjs" && importer?.includes("libsodium-wrappers")) {
      return libsodiumEsm;
    }
    return null;
  },
};

// The bundle is served same-origin by skriftapp's BuildController:
//   /app/          → static/index.html (with {{USER0}} substituted)
//   /app/static/*  → static/* (hashed assets: js, css, fonts)
// Every dependency is bundled locally — no CDN at runtime.
export default defineConfig({
  base: "/app/static/",
  plugins: [libsodiumShim],
  build: {
    outDir: "../skriftapp/buildapp/static",
    emptyOutDir: true,
    target: "es2022",
  },
});
