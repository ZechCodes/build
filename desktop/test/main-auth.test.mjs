import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

const flush = async () => {
  for (let index = 0; index < 6; index++) await new Promise(setImmediate);
};
const tokens = (suffix = "rotated") => ({
  access_token: `fixture-access-${suffix}`,
  refresh_token: `fixture-refresh-${suffix}`,
  expires_in: 900,
});
const response = (payload, ok = true) => ({ ok, json: async () => payload });
let sequence = 0;

async function launch(t, { stored = "fixture-refresh-saved", available = true, fetch } = {}) {
  const directory = mkdtempSync(join(tmpdir(), "desktop-main-auth-"));
  const path = join(directory, "refresh-token.enc");
  if (stored) writeFileSync(path, `encrypted:${stored}`, { mode: 0o600 });
  const calls = { browser: [], fetch: [], windows: [], dialogs: [], timers: [] };
  const app = new EventEmitter();
  Object.assign(app, {
    isPackaged: false,
    requestSingleInstanceLock: () => true,
    setAppUserModelId() {}, setAsDefaultProtocolClient() {},
    whenReady: async () => {},
    getPath: (name) => { assert.equal(name, "userData"); return directory; },
    quit: () => { calls.quit = (calls.quit || 0) + 1; },
  });
  class BrowserWindow extends EventEmitter {
    constructor(options) {
      super();
      this.options = options;
      this.webContents = new EventEmitter();
      this.webContents.setWindowOpenHandler = () => {};
      calls.windows.push(this);
    }
    loadURL(url) { this.url = url; return Promise.resolve(); }
    show() {} focus() {} isMinimized() { return false; }
    destroy() { this.destroyed = true; this.emit("closed"); }
  }
  const fixture = {
    app, BrowserWindow,
    safeStorage: {
      isEncryptionAvailable: () => available,
      getSelectedStorageBackend: () => "gnome_libsecret",
      encryptString: (value) => Buffer.from(`encrypted:${value}`),
      decryptString: (bytes) => bytes.toString().slice(10),
    },
    dialog: { showMessageBox: async (options) => {
      calls.dialogs.push(options);
      return { response: calls.dialogResponse ?? 1 };
    } },
    Menu: { buildFromTemplate: (template) => template, setApplicationMenu: (menu) => { calls.menu = menu; } },
    net: { fetch: async (url, options) => {
      calls.fetch.push({ url, body: new URLSearchParams(options.body) });
      return fetch ? fetch(calls.fetch.length) : response(tokens());
    } },
    session: { defaultSession: {
      setPermissionCheckHandler() {}, setPermissionRequestHandler() {},
      webRequest: { onBeforeSendHeaders: (_filter, handler) => { calls.headers = handler; } },
    } },
    shell: { openExternal: async (url) => { calls.browser.push(url); } },
  };
  const id = ++sequence;
  const key = `__desktopElectron${id}`;
  globalThis[key] = fixture;
  const hook = registerHooks({
    resolve(specifier, context, nextResolve) {
      if (specifier === "electron") return { url: `stub:electron-${id}`, shortCircuit: true };
      return nextResolve(specifier, context);
    },
    load(url, context, nextLoad) {
      if (url === `stub:electron-${id}`) return {
        format: "module", shortCircuit: true,
        source: `export const { app, BrowserWindow, safeStorage, dialog, Menu, net, session, shell } = globalThis.${key};`,
      };
      return nextLoad(url, context);
    },
  });
  t.mock.timers.enable({ apis: ["setTimeout"] });
  t.after(() => {
    hook.deregister();
    delete globalThis[key];
    rmSync(directory, { recursive: true, force: true });
  });
  await import(`../src/main.mjs?test=${id}`);
  await flush();
  const header = () => {
    let result;
    calls.headers({ url: "https://getbuild.ing/api/devices", resourceType: "xhr", requestHeaders: {} }, (value) => { result = value.requestHeaders; });
    return result.Authorization;
  };
  const signOut = () => calls.menu.flatMap((item) => item.submenu || []).find((item) => item.label === "Sign Out");
  const callback = () => {
    const state = new URL(calls.browser.at(-1)).searchParams.get("state");
    app.emit("open-url", { preventDefault() {} }, `getbuilding://oauth/callback?code=fixture-code&state=${state}`);
  };
  return { app, calls, path, header, signOut, callback };
}

test("launch silently refreshes saved credentials and rotates before opening the app", async (t) => {
  const { calls, path, header } = await launch(t);
  assert.deepEqual(calls.browser, []);
  assert.equal(calls.fetch.length, 1);
  assert.equal(calls.fetch[0].body.get("refresh_token"), "fixture-refresh-saved");
  assert.equal(calls.fetch[0].body.get("client_id"), "build-desktop");
  assert.equal(calls.windows.length, 1);
  assert.equal(header(), "Bearer fixture-access-rotated");
  assert.equal(readFileSync(path, "utf8"), "encrypted:fixture-refresh-rotated");
});

test("first sign-in persists credentials and follows the existing PKCE callback flow", async (t) => {
  const { calls, path, callback } = await launch(t, { stored: null });
  assert.equal(calls.browser.length, 1);
  assert.equal(calls.fetch.length, 0);
  callback();
  await flush();
  assert.equal(calls.fetch[0].body.get("grant_type"), "authorization_code");
  assert.ok(calls.fetch[0].body.get("code_verifier"));
  assert.equal(readFileSync(path, "utf8"), "encrypted:fixture-refresh-rotated");
  assert.equal(calls.windows.length, 1);
});

test("without encryption sign-in stays in memory and never writes plaintext", async (t) => {
  const { calls, path, callback, header } = await launch(t, { stored: null, available: false });
  callback();
  await flush();
  assert.equal(header(), "Bearer fixture-access-rotated");
  assert.equal(calls.windows.length, 1);
  assert.equal(existsSync(path), false);
});

test("invalid_grant clears saved credentials and opens browser sign-in", async (t) => {
  const { calls, path, header } = await launch(t, {
    fetch: () => response({ error: "invalid_grant", error_description: "fixture refusal" }, false),
  });
  assert.equal(calls.fetch.length, 1);
  assert.equal(existsSync(path), false);
  assert.equal(header(), undefined);
  assert.equal(calls.browser.length, 1);
  assert.equal(calls.windows.length, 0);
});

for (const [name, fetch] of [
  ["transport outage", () => { throw new Error("fixture offline"); }],
  ["server error", () => response({ error: "server_error" }, false)],
  ["incomplete response", () => response({ access_token: "fixture-incomplete" })],
]) {
  test(`${name} retains saved token and never opens browser sign-in`, async (t) => {
    const { calls, path } = await launch(t, { fetch });
    assert.equal(readFileSync(path, "utf8"), "encrypted:fixture-refresh-saved");
    assert.equal(calls.browser.length, 0);
    assert.equal(calls.windows.length, 0);
    assert.equal(calls.dialogs.length, 1);
  });
}

test("Try Again on a startup outage retries refresh instead of browser sign-in", async (t) => {
  let attempts = 0;
  // Hold the first request until the launch fixture is available to select retry.
  let rejectFirst;
  const first = new Promise((_resolve, reject) => { rejectFirst = reject; });
  const { calls, path } = await launch(t, { fetch: () => ++attempts === 1 ? first : response(tokens("retried")) });
  calls.dialogResponse = 0;
  rejectFirst(new Error("fixture offline"));
  await flush();
  assert.equal(calls.fetch.length, 2);
  assert.equal(calls.browser.length, 0);
  assert.equal(readFileSync(path, "utf8"), "encrypted:fixture-refresh-retried");
});

test("sign-out clears disk, headers and window, and cancels scheduled refresh", async (t) => {
  const { calls, path, header, signOut } = await launch(t);
  assert.ok(signOut(), "native menu has Sign Out");
  await signOut().click();
  await flush();
  assert.equal(existsSync(path), false);
  assert.equal(header(), undefined);
  assert.equal(calls.windows[0].destroyed, true);
  assert.equal(calls.quit, 1);
  t.mock.timers.tick(900_000);
  await flush();
  assert.equal(calls.fetch.length, 1);
});

test("activation during startup refresh cannot open a browser or duplicate refresh", async (t) => {
  let resolveRefresh;
  const pending = new Promise((resolve) => { resolveRefresh = resolve; });
  const { app, calls } = await launch(t, { fetch: () => pending });
  app.emit("activate");
  app.emit("activate");
  await flush();
  assert.equal(calls.fetch.length, 1);
  assert.deepEqual(calls.browser, []);
  resolveRefresh(response(tokens()));
  await flush();
  assert.equal(calls.windows.length, 1);
});

test("sign-out while refresh is pending ignores its late rotated credentials", async (t) => {
  let resolveRefresh;
  const pending = new Promise((resolve) => { resolveRefresh = resolve; });
  const { calls, path, header, signOut } = await launch(t, {
    fetch: (count) => count === 1 ? response(tokens()) : pending,
  });
  t.mock.timers.tick(840_000);
  await flush();
  assert.equal(calls.fetch.length, 2);
  await signOut().click();
  resolveRefresh(response(tokens("late")));
  await flush();
  assert.equal(existsSync(path), false);
  assert.equal(header(), undefined);
  assert.equal(calls.windows.length, 1);
  t.mock.timers.tick(900_000);
  await flush();
  assert.equal(calls.fetch.length, 2);
});

test("sign-out ignores a late authorization-code exchange", async (t) => {
  let resolveExchange;
  const pending = new Promise((resolve) => { resolveExchange = resolve; });
  const { calls, path, callback, signOut, header } = await launch(t, { stored: null, fetch: () => pending });
  callback();
  await flush();
  await signOut().click();
  resolveExchange(response(tokens("late")));
  await flush();
  assert.equal(existsSync(path), false);
  assert.equal(header(), undefined);
  assert.equal(calls.windows.length, 0);
});
