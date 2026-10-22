import { build } from "vite";
import { afterAll, beforeAll, expect, it } from "vitest";
import { openLayoutSession } from "./layoutHarness.mjs";

let session;
let assets;

beforeAll(async () => {
  // Exercise the real entry and production chunk graph. Vite's dev optimizer
  // can hide side-effect ordering failures in CommonJS grammar registration.
  const bundle = await build({ logLevel: "silent", build: { write: false } });
  assets = new Map(bundle.output.map((asset) => [asset.fileName,
    asset.type === "chunk" ? asset.code : asset.source]));
  session = await openLayoutSession();
});
afterAll(async () => { await session?.close(); });

it.each([393, 1280])("boots the production app at %ipx without module errors", async (width) => {
  await session.withPage(async ({ page }) => {
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.context().unrouteAll();
    await page.route("https://build.fixture.test/**", async (route) => {
      const path = new URL(route.request().url()).pathname;
      if (path === "/api/devices") {
        return route.fulfill({ json: { devices: [] } });
      }
      const name = path === "/app/" ? "index.html" : path.replace("/app/static/", "");
      const body = assets.get(name);
      if (body === undefined) return route.fulfill({ status: 404, body: "" });
      const contentType = name.endsWith(".js") ? "text/javascript"
        : name.endsWith(".css") ? "text/css"
          : name.endsWith(".html") ? "text/html" : "application/octet-stream";
      return route.fulfill({ contentType, body });
    });
    await page.goto("https://build.fixture.test/app/");
    await expect.poll(async () => errors.length > 0
      || await page.getByRole("heading", { name: "Welcome to Build" }).count() > 0).toBe(true);
    expect(errors).toEqual([]);
    expect(await page.getByRole("heading", { name: "Welcome to Build" }).isVisible()).toBe(true);
    expect(await page.getByRole("button", { name: "Add device", exact: true }).isEnabled()).toBe(true);
  }, { width, height: 852 });
});
