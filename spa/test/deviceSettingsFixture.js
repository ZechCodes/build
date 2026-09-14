// The device's own settings page, as the suites that read one of its panels
// render it.
//
// Rendering it means standing up the app, one paired machine and a bridge that
// answers for it — the same eight lines in every suite that reads a panel on
// this page, so they are minted here instead.
//
// A suite still writes its own `vi.mock("../src/connection.js", ...)` factory:
// mockedExports.test.js reads those factories back to check every name against
// the real module, and only sees an object literal written in the suite.

import { vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { sessionAnswering } from "./deviceSessionFixture.js";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const flush = () => new Promise((done) => setTimeout(done, 0));

/**
 * Render one paired machine's settings page against a bridge that answers
 * `call`, and hand back the app it rendered on.
 *
 * The page's own connection and the account's context for the same machine both
 * answer with that bridge, so a suite writes one and every reader sees the same
 * one. `openSession` is the suite's spy for the page's opener — the same one it
 * handed the connection-module factory.
 */
export async function renderDeviceSettingsPage(call, openSession) {
  vi.resetModules();
  document.body.innerHTML = bodyHtml;
  const { App } = await import("../src/app.js");
  const { adoptDeviceSession } = await import("../src/core/deviceContexts.js");
  const { renderDeviceSettings } = await import("../src/views/deviceSettings.js");
  App.call = vi.fn(call);
  App.devices = [{ id: "dev-1", name: "Laptop", status: "online" }];
  App.selectedDeviceId = "dev-1";
  App.route = { name: "device", id: "dev-1" };
  openSession.mockResolvedValue(sessionAnswering(App));
  adoptDeviceSession(sessionAnswering(App));
  await renderDeviceSettings();
  await flush();
  return App;
}
