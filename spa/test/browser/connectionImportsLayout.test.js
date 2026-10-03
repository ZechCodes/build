import { describe, expect, it } from "vitest";
import { loadBrowserModules, withLayoutPage } from "./layoutHarness.mjs";

describe("connection imports in a fresh browser module graph", () => {
  it.each(["src/connection.js", "src/core/deviceContexts.js", "src/app.js"])(
    "preserves shared security stops when %s loads first",
    async (entry) => {
      await withLayoutPage(async ({ page, basePath }) => {
        // Load the entry alone: concurrent imports can hide the cycle depending
        // on which response reaches the browser first, as in the Inbox captures.
        await loadBrowserModules(page, { entry }, basePath);
        await page.evaluate(() => { delete window.__layoutModules; });
        await loadBrowserModules(page, {
          connection: "src/connection.js",
          contexts: "src/core/deviceContexts.js",
        }, basePath);

        const stops = await page.evaluate(() => {
          const { connection, contexts } = window.__layoutModules;
          const read = () => [connection.securityStopText(), contexts.deviceSecurityStopText()];
          const initial = read();
          contexts.refuseDeviceConnection("dev-import", "Pinned key changed");
          const refused = read();
          connection.forgetSecurityStops();
          const cleared = read();
          contexts.refuseDeviceConnection("dev-import", "Device refused");
          contexts.resetDeviceContexts();
          return { initial, refused, cleared, reset: read() };
        });

        expect(stops).toEqual({
          initial: ["", ""],
          refused: ["Pinned key changed", "Pinned key changed"],
          cleared: ["", ""],
          reset: ["", ""],
        });
      });
    },
  );
});
