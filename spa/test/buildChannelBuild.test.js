// @vitest-environment jsdom
import { resolve } from "node:path";
import { build } from "vite";
import { expect, it } from "vitest";

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
  const code = output.output.find((file) => file.type === "chunk").code;
  expect(code).not.toContain("import.meta.env");
  document.body.innerHTML = '<span id="build-channel" hidden>Dev</span>';
  document.title = "Build — review is the product";
  const compiled = await import(/* @vite-ignore */ `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
  compiled.initBuildChannel();
  expect(document.querySelector("#build-channel").hidden).toBe(channel !== "dev");
  expect(document.title).toBe(channel === "dev" ? "Build Dev — review is the product" : "Build — review is the product");
});
