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

// The version the build is stamped as — CI passes the git SHA; a build with
// none is a dev build, and the client's version watcher stays off for those.
// One env var feeds both sides of the comparison: import.meta.env in the
// bundle, and the version.json the plugin below emits beside it.
const buildVersion = process.env.VITE_BUILD_VERSION || "dev";

/** Emit static/version.json naming the version this output was built as, so
 *  a running client can ask the server what it is currently serving. */
export function versionStampPlugin(version) {
  return {
    name: "build-version-stamp",
    generateBundle() {
      this.emitFile({
        type: "asset",
        fileName: "version.json",
        source: JSON.stringify({ version }),
      });
    },
  };
}

// The bundle is served same-origin by skriftapp's BuildController:
//   /app/          → static/index.html (with {{USER0}} substituted)
//   /app/static/*  → static/* (hashed assets: js, css, fonts)
// Every dependency is bundled locally — no CDN at runtime.
export default defineConfig({
  base: "/app/static/",
  plugins: [libsodiumShim, versionStampPlugin(buildVersion)],
  build: {
    outDir: "../skriftapp/buildapp/static",
    emptyOutDir: true,
    target: "es2022",
  },
});
