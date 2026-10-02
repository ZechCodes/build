import { fileURLToPath } from "node:url";
import { build } from "vite";

// A separate bundle graph is deliberate: sharing gsap, stage or Vite's
// preload helper would rewrite the home page's existing script and hashes.
export function heroPreviewBuild() {
  return {
    name: "hero336-preview",
    hooks: {
      "astro:build:done": async ({ dir }) => {
        await build({
          configFile: false,
          base: "/landing/generated/hero336/",
          build: {
            outDir: fileURLToPath(new URL("hero336/", dir)),
            emptyOutDir: true,
            assetsInlineLimit: 0,
            rollupOptions: {
              input: fileURLToPath(new URL("./main.js", import.meta.url)),
              output: { entryFileNames: "preview.js" },
            },
          },
        });
      },
    },
  };
}
