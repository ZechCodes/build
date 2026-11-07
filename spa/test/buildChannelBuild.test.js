// @vitest-environment jsdom
import { createHash } from "node:crypto";
import { mkdtemp, readFile, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";
import { build } from "vite";
import { afterAll, afterEach, describe, expect, it, vi } from "vitest";

it.each(["dev", "prod"])("bakes %s into an output that needs no runtime environment", async (channel) => {
  const output = await build({
    configFile: false,
    logLevel: "silent",
    define: { "import.meta.env.VITE_BUILD_CHANNEL": JSON.stringify(channel) },
    build: {
      write: false,
      lib: { entry: resolve("src/core/buildChannel.js"), formats: ["es"] },
    },
  });
  const bundle = Array.isArray(output) ? output[0] : output;
  const code = bundle.output.find((file) => file.type === "chunk").code;
  expect(code).not.toContain("import.meta.env");
  document.body.innerHTML = '<button class="connection-status"></button>';
  document.title = "Build — review is the product";
  const compiled = await import(/* @vite-ignore */ `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
  compiled.initBuildChannel();
  const button = document.querySelector(".connection-status");
  compiled.mountBuildChannel(button);
  expect(Boolean(button.querySelector(".connection-build-channel"))).toBe(channel === "dev");
  expect(button.getAttribute("aria-description")).toBe(channel === "dev" ? "Dev build" : null);
  expect(document.title).toBe(channel === "dev" ? "Build Dev — review is the product" : "Build — review is the product");
});

// Pin the production assets independently of the working tree: changing both
// the source and output must not silently redefine the production icon.
const productionHashes = {
  "favicon.svg": "193cf6a9c688e30bde06d85de0c393595f3307cceb80a34d25073376b9dbdc7d",
  "apple-touch-icon.png": "efac7fbdb6c7f230384218d28e73ec21740f654abfd5db482a92a53a6d87938b",
  "icon-192.png": "08da309b8345126043791ef46a297aea89507596818a005a693b34cfc3791ec8",
  "icon-512.png": "b9c23970bfba14dd85de1192dadcf32e2fb93dbffb82cb4c6c429daaa9497928",
  "manifest.webmanifest": "c525d20e9a3bc92ce656a7f5cf404c0535276c9ee13c6392827699e1411b71dc",
};

describe("channel icons in the built SPA", () => {
  let scratch;
  afterEach(() => vi.unstubAllEnvs());
  afterAll(async () => {
    if (scratch) await rm(scratch, { recursive: true, force: true });
  });

  async function buildChannel(channel) {
    scratch ||= await mkdtemp(join(tmpdir(), "build-channel-icons-"));
    vi.stubEnv("VITE_BUILD_CHANNEL", channel);
    const outDir = join(scratch, channel || "unset");
    await build({
      configFile: resolve("vite.config.js"),
      logLevel: "silent",
      build: { outDir },
    });
    return outDir;
  }

  it("emits the inverted dev favicon and wires tab, touch and manifest icons at build time", async () => {
    const outDir = await buildChannel("dev");
    const html = await readFile(join(outDir, "index.html"), "utf8");
    document.documentElement.innerHTML = html;
    expect(document.querySelector('link[rel="manifest"]').getAttribute("href"))
      .toBe("/app/static/manifest-dev.webmanifest");
    expect([...document.querySelectorAll('link[rel="icon"]')].map((link) => link.getAttribute("href")))
      .toEqual(["/app/static/icon-dev-192.png", "/app/static/favicon-dev.svg"]);
    expect(document.querySelector('link[rel="apple-touch-icon"]').getAttribute("href"))
      .toBe("/app/static/apple-touch-icon-dev.png");
    expect(document.querySelector('meta[name="apple-mobile-web-app-title"]').content).toBe("Build Dev");

    const original = await readFile("public/favicon.svg", "utf8");
    const inverted = original.replaceAll("#5EF5B5", "#INVERT").replaceAll("#000000", "#5EF5B5")
      .replaceAll("#INVERT", "#000000");
    expect(await readFile(join(outDir, "favicon-dev.svg"), "utf8")).toBe(inverted);
    const manifest = JSON.parse(await readFile(join(outDir, "manifest-dev.webmanifest"), "utf8"));
    const production = JSON.parse(await readFile("public/manifest.webmanifest", "utf8"));
    expect(manifest).toEqual({
      ...production,
      name: "Build Dev",
      short_name: "Build Dev",
      icons: production.icons.map((icon) => ({ ...icon, src: icon.src.replace("icon-", "icon-dev-") })),
    });
    // Installed PWAs retain their manifest URL; existing tabs and the push
    // worker can also request the original icon URLs after a dev deployment.
    for (const [original, dev] of [["manifest.webmanifest", "manifest-dev.webmanifest"],
      ["favicon.svg", "favicon-dev.svg"], ["icon-192.png", "icon-dev-192.png"],
      ["icon-512.png", "icon-dev-512.png"], ["apple-touch-icon.png", "apple-touch-icon-dev.png"]]) {
      expect(await readFile(join(outDir, original)), original).toEqual(await readFile(join(outDir, dev)));
    }

    // A real browser decoder validates the complete PNG stream, then samples
    // both flat fills, rather than trusting just the dimensions in its header.
    const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH || "/usr/bin/chromium", headless: true });
    try {
      const page = await browser.newPage();
      for (const [file, size] of [["favicon-dev.png", 32], ["apple-touch-icon-dev.png", 180],
        ["icon-dev-192.png", 192], ["icon-dev-512.png", 512]]) {
        const png = await readFile(join(outDir, file));
        const decoded = await page.evaluate(async (base64) => {
          const bytes = Uint8Array.from(atob(base64), (char) => char.charCodeAt(0));
          const image = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
          const canvas = document.createElement("canvas");
          canvas.width = image.width;
          canvas.height = image.height;
          const context = canvas.getContext("2d");
          context.drawImage(image, 0, 0);
          const pixel = (x, y) => [...context.getImageData(x, y, 1, 1).data];
          return { width: image.width, height: image.height, background: pixel(0, 0),
            letter: pixel(Math.floor(image.width * 0.57), Math.floor(image.height * 0.59)) };
        }, png.toString("base64"));
        expect(decoded, file).toEqual({ width: size, height: size, background: [0, 0, 0, 255], letter: [94, 245, 181, 255] });
      }
    } finally {
      await browser.close();
    }
  }, 120_000);

  it.each(["prod", undefined])("preserves production icon bytes and URLs for channel %s", async (channel) => {
    const outDir = await buildChannel(channel);
    for (const [file, hash] of Object.entries(productionHashes)) {
      expect(createHash("sha256").update(await readFile(join(outDir, file))).digest("hex"), file).toBe(hash);
    }
    expect((await readdir(outDir)).filter((file) => file.includes("-dev"))).toEqual([]);
    const html = await readFile(join(outDir, "index.html"), "utf8");
    document.documentElement.innerHTML = html;
    const original = await readFile("index.html", "utf8");
    const source = new DOMParser().parseFromString(original, "text/html");
    for (const selector of ['link[rel="manifest"]', 'link[rel="icon"]', 'link[rel="apple-touch-icon"]',
      'meta[name="apple-mobile-web-app-title"]']) {
      expect([...document.querySelectorAll(selector)].map((node) => node.outerHTML))
        .toEqual([...source.querySelectorAll(selector)].map((node) => node.outerHTML));
    }
  }, 120_000);
});
