import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";

export const flush = async () => {
  for (let index = 0; index < 6; index++) await new Promise(setImmediate);
};
export const tokens = (suffix = "rotated") => ({
  access_token: `fixture-access-${suffix}`,
  refresh_token: `fixture-refresh-${suffix}`,
  expires_in: 900,
});
export const response = (payload, ok = true) => ({ ok, json: async () => payload });
let sequence = 0;

export async function launch(t, { stored = "fixture-refresh-saved", available = true, fetch, userData, openExternal, safeStorage = {}, clearStorageData, clearCache, removeSavedToken, showMessageBox, loadURL, bridgeFetch } = {}) {
  if (bridgeFetch) t.mock.method(globalThis, "fetch", bridgeFetch);
  const directory = userData || mkdtempSync(join(tmpdir(), "desktop-main-auth-"));
  const path = join(directory, "refresh-token.enc");
  if (stored && !userData) writeFileSync(path, `encrypted:${stored}`, { mode: 0o600 });
  const calls = { browser: [], fetch: [], windows: [], dialogs: [], timers: [], cleanup: [], quitEvents: [], reportWindows: [], partitions: [], ipc: new Map(), bridgeEvents: [] };
  const app = new EventEmitter();
  Object.assign(app, {
    isPackaged: false,
    requestSingleInstanceLock: () => true,
    setAppUserModelId() {}, setAsDefaultProtocolClient() {},
    whenReady: async () => {},
    getPath: (name) => { assert.equal(name, "userData"); return directory; },
    quit: () => {
      const event = { prevented: false, preventDefault() { this.prevented = true; } };
      app.emit("before-quit", event);
      calls.quitEvents.push(event.prevented);
      if (!event.prevented) calls.quit = (calls.quit || 0) + 1;
    },
  });
  class BrowserWindow extends EventEmitter {
    constructor(options) {
      super();
      this.options = options;
      this.actions = [];
      this.webContents = new EventEmitter();
      this.webContents.id = calls.windows.length + 1;
      this.webContents.mainFrame = { url: "https://getbuild.ing/app/desktop" };
      this.webContents.send = (...args) => calls.bridgeEvents.push(args);
      this.webContents.setWindowOpenHandler = (handler) => { this.webContents.openWindow = handler; };
      calls.windows.push(this);
    }
    loadURL(url) {
      this.actions.push("load"); this.url = url;
      this.webContents.emit("did-start-navigation", {}, url, false, true);
      return loadURL ? loadURL(this, url) : Promise.resolve();
    }
    hide() { this.actions.push("hide"); }
    show() { this.actions.push("show"); }
    focus() { this.actions.push("focus"); }
    isMinimized() { return Boolean(this.minimized); }
    restore() { this.actions.push("restore"); this.minimized = false; }
    destroy() { this.actions.push("destroy"); this.destroyed = true; this.emit("closed"); }
  }
  class BaseWindow extends EventEmitter {
    constructor(options) { super(); this.options = options; calls.reportWindows.push(this); }
    isDestroyed() { return Boolean(this.destroyed); }
    destroy() { assert.equal(this.isDestroyed(), false, "native window cannot be destroyed twice"); this.destroyed = true; }
  }
  const defaultState = { cookie: "account-cookie", cache: "account-content" };
  const tokenState = { cookie: null, cache: null };
  const fetchInSession = async (state, url, options) => {
    calls.fetch.push({ url, body: new URLSearchParams(options.body), options, state });
    const result = fetch ? await fetch(calls.fetch.length, options) : response(tokens());
    // Model a late network response even if cancellation loses the race.
    if (result.setCookie && options.credentials !== "omit") state.cookie = result.setCookie;
    if (result.cachedContent && options.cache !== "no-store") state.cache = result.cachedContent;
    return result;
  };
  const fixture = {
    app, BrowserWindow, BaseWindow, removeSavedToken,
    safeStorage: {
      isEncryptionAvailable: () => available,
      getSelectedStorageBackend: () => "gnome_libsecret",
      encryptString: (value) => Buffer.from(`encrypted:${value}`),
      decryptString: (bytes) => bytes.toString().slice(10),
      ...safeStorage,
    },
    dialog: { showMessageBox: async (parentOrOptions, parentOptions) => {
      const options = parentOptions || parentOrOptions;
      if (options.title === "Build sign-out failed") {
        assert.ok(parentOrOptions instanceof BaseWindow, "macOS needs a parent for an asynchronous dialog");
        assert.ok(options.signal instanceof AbortSignal, "failure dialog can be closed at its deadline");
        calls.reportSignal = options.signal;
      }
      calls.dialogs.push(options);
      return showMessageBox ? showMessageBox(options, parentOptions ? parentOrOptions : undefined) : { response: calls.dialogResponse ?? 1 };
    } },
    ipcMain: { handle: (channel, handler) => calls.ipc.set(channel, handler) },
    Menu: { buildFromTemplate: (template) => template, setApplicationMenu: (menu) => { calls.menu = menu; } },
    net: { fetch: async (url, options) => {
      return fetchInSession(defaultState, url, options);
    } },
    session: {
      fromPartition: (partition, options) => {
        calls.partitions.push({ partition, options });
        assert.ok(partition && !partition.startsWith("persist:"), "token session must be in memory");
        assert.equal(options.cache, false, "token session disables HTTP caching");
        return { fetch: (url, options) => fetchInSession(tokenState, url, options) };
      },
      defaultSession: {
      clearStorageData: async (...args) => {
        calls.cleanup.push("storage");
        assert.deepEqual(args, [], "clear every default-session storage type");
        assert.ok(calls.windows.every((window) => window.destroyed), "renderer must retire first");
        if (clearStorageData) await clearStorageData();
        defaultState.cookie = null;
      },
      clearCache: async () => {
        calls.cleanup.push("cache");
        assert.ok(calls.windows.every((window) => window.destroyed), "renderer must retire first");
        if (clearCache) await clearCache();
        defaultState.cache = null;
      },
      setPermissionCheckHandler() {}, setPermissionRequestHandler() {},
      webRequest: {
        onBeforeSendHeaders: (_filter, handler) => { calls.headers = handler; },
        onHeadersReceived: (filter, handler) => { calls.responseFilter = filter; calls.responseHeaders = handler; },
      },
    } },
    shell: { openExternal: async (url) => {
      calls.browser.push(url);
      if (openExternal) return openExternal(calls.browser.length);
    } },
  };
  const id = ++sequence;
  const key = `__desktopElectron${id}`;
  globalThis[key] = fixture;
  const hook = registerHooks({
    resolve(specifier, context, nextResolve) {
      if (specifier === "./token-store.mjs" && context.parentURL === new URL(`../../src/main.mjs?test=${id}`, import.meta.url).href) {
        const resolved = nextResolve(specifier, context);
        return { ...resolved, url: `${resolved.url}?test=${id}` };
      }
      if (specifier === "./bridge-http-resources.json") return { url: `stub:bridge-manifest-${id}`, shortCircuit: true };
      if (specifier === "electron") return { url: `stub:electron-${id}`, shortCircuit: true };
      return nextResolve(specifier, context);
    },
    load(url, context, nextLoad) {
      // Main auth/IPC tests exercise lifecycle only. Production manifest parity
      // is checked separately against the independently reviewed #915 artifact.
      if (url === `stub:bridge-manifest-${id}`) return { format: "json", shortCircuit: true, source: '{"version":1,"operations":[]}' };
      if (url === `stub:electron-${id}`) return {
        format: "module", shortCircuit: true,
        source: `export const { app, BrowserWindow, BaseWindow, safeStorage, dialog, Menu, net, session, shell, ipcMain } = globalThis.${key};`,
      };
      const loaded = nextLoad(url, context);
      if (url === new URL(`../../src/main.mjs?test=${id}`, import.meta.url).href) {
        return { ...loaded, source: `${loaded.source}\nexport {
          refreshTokens as testRefreshTokens,
          clearCredentials as testClearCredentials,
          showAuthenticationError as testShowAuthenticationError,
          openAuthenticatedWindow as testOpenAuthenticatedWindow,
          startAuthentication as testStartAuthentication,
          beginAuthentication as testBeginAuthentication,
        };` };
      }
      if (removeSavedToken && url === new URL(`../../src/token-store.mjs?test=${id}`, import.meta.url).href) {
        return { ...loaded, source: String(loaded.source).replace(
          'const clear = () => rmSync(path, { force: true });',
          `const clear = () => globalThis.${key}.removeSavedToken();`,
        ) };
      }
      return loaded;
    },
  });
  if (!userData) t.mock.timers.enable({ apis: ["setTimeout"] });
  t.after(() => {
    hook.deregister();
    delete globalThis[key];
    rmSync(directory, { recursive: true, force: true });
  });
  const {
    testRefreshTokens, testShowAuthenticationError, testOpenAuthenticatedWindow,
    testStartAuthentication, testBeginAuthentication, testClearCredentials,
  } = await import(`../../src/main.mjs?test=${id}`);
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
  return {
    app, calls, defaultState, tokenState, path, header, signOut, callback,
    refreshTokens: testRefreshTokens, clearCredentials: testClearCredentials, showAuthenticationError: testShowAuthenticationError,
    openAuthenticatedWindow: testOpenAuthenticatedWindow,
    startAuthentication: testStartAuthentication, beginAuthentication: testBeginAuthentication,
    safeStorage: fixture.safeStorage,
  };
}
