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

/** A versioned bundle is deployable, so it must never inherit the browser-only
 * local relay fallback. Development builds keep the zero-config localhost
 * path used by `npm run dev` and local previews. */
export function validateDeployRelay({ buildVersion, relayUrl }) {
  if (buildVersion === "dev") return;
  if (!relayUrl) {
    throw new Error("VITE_RELAY_URL is required when VITE_BUILD_VERSION stamps a deployable SPA build");
  }
  const relay = new URL(relayUrl);
  if (["localhost", "127.0.0.1", "::1", "[::1]"].includes(relay.hostname)) {
    throw new Error("VITE_RELAY_URL must not target localhost in a deployable SPA build");
  }
}

validateDeployRelay({
  buildVersion,
  relayUrl: process.env.VITE_RELAY_URL,
});

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
  test: {
    // The DOM suites open with `await import("../src/app.js")`, which makes
    // the worker transform and execute the whole client module graph before
    // the first assertion. That is work, not waiting — and with the suite's
    // files running in parallel on a busy machine the work alone outran
    // vitest's 10 s hook / 5 s test defaults, failing a different handful of
    // files on every run. The budgets below are sized for the work so a
    // timeout means something is genuinely stuck.
    hookTimeout: 60_000,
    testTimeout: 20_000,
    // Node 22+ defines a global `localStorage` that is unusable without
    // `--localstorage-file`, and its presence stops the jsdom environment
    // from installing its own. Off, so the DOM suites get jsdom's.
    execArgv: ["--no-experimental-webstorage"],
  },
});
