import assert from "node:assert/strict";
import { readFile, rename, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { createServer } from "../../spa/node_modules/vite/dist/node/index.js";
import { chromium } from "../../spa/node_modules/playwright-core/index.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const artifacts = process.argv[2];
const save = async (name, value) => {
  const target = path.join(artifacts, name);
  await writeFile(`${target}.tmp`, typeof value === "string" ? value : JSON.stringify(value, null, 2));
  await rename(`${target}.tmp`, target);
};
const server = await createServer({
  root: path.resolve(here, "../../spa"),
  configFile: false,
  server: { host: "127.0.0.1", port: 9001, strictPort: true, hmr: false, watch: null,
    fs: { allow: [path.resolve(here, "../../..")] } },
  plugins: [{ name: "lan-fixture", configureServer(vite) {
    vite.middlewares.use("/fixture", async (_request, response) => {
      response.setHeader("Content-Type", "text/html");
      response.end(`<script type="module" src="/@fs/${here}/browser.mjs"></script>`);
    });
  } }],
});
let browser;
let page;
try {
  await server.listen();
  browser = await chromium.launch({ executablePath: "/usr/bin/chromium", headless: true,
    args: ["--no-sandbox", "--disable-dev-shm-usage"] });
  page = await browser.newPage();
  page.on("console", (event) => console.log(event.type(), event.text()));
  page.on("pageerror", (error) => console.error("page error", error));
  await page.exposeFunction("fixtureReady", () => save("browser-ready", "ready"));
  await page.exposeFunction("fixtureHost", async (host) => {
    await save("host.json", host);
    await save("gate", "ready");
    while (true) {
      try { await readFile(path.join(artifacts, "gated")); break; } catch (error) {
        if (error.code !== "ENOENT") throw error;
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
    }
  });
  await page.exposeFunction("fixtureRelease", async (state) => {
    console.log("TURN carrying", JSON.stringify(state));
    await save("before.json", state);
    await save("release", "ready");
  });
  await page.exposeFunction("fixtureChecks", async () => {
    const wire = JSON.parse(await readFile(path.join(artifacts, "wire.json"), "utf8"));
    return wire.host_checks.filter((row) => row.after_release).length;
  });
  await page.exposeFunction("fixtureReleaseChecks", () => save("release-host-checks", "ready"));
  await page.exposeFunction("fixtureViable", (state) => {
    console.log("direct succeeded before restart", JSON.stringify(state));
    return save("viable.json", state);
  });
  await page.goto("http://localhost:9001/fixture");
  console.log("fixture page loaded");
  await page.waitForFunction(() => typeof window.runLanUpgrade === "function");
  console.log("fixture modules ready");
  const deadline = Date.now() + 15000;
  while (true) {
    try { await readFile(path.join(artifacts, "firewall-ready")); break; } catch (error) {
      if (error.code !== "ENOENT" || Date.now() > deadline) throw error;
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
  }
  const result = await page.evaluate(() => window.runLanUpgrade());
  const wire = JSON.parse(await readFile(path.join(artifacts, "wire.json"), "utf8"));
  assert(wire.host_checks.some((row) => row.after_release), "bridge sends outbound late-host STUN checks");
  assert(wire.queries.length > 0 && wire.queries.every((query) => query.port === 5353 && query.class === 1),
    "bridge queries mDNS from5353 usingQM");
  await save("result.json", { ...result, wire });
  console.log("PASS", JSON.stringify({ before: result.before, viable: result.viable, after: result.after, wire }));
} catch (error) {
  const snapshot = await page?.evaluate(() => window.fixtureSnapshot?.()).catch(() => null);
  await save("failure.json", { error: String(error), snapshot });
  console.error("fixture failure", JSON.stringify(snapshot));
  console.error(error);
  process.exitCode = 1;
} finally {
  await browser?.close();
  await server.close();
}
