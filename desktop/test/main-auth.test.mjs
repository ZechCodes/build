import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
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

async function launch(t, { stored = "fixture-refresh-saved", available = true, fetch, userData, openExternal, safeStorage = {}, clearStorageData, clearCache, removeSavedToken, showMessageBox } = {}) {
  const directory = userData || mkdtempSync(join(tmpdir(), "desktop-main-auth-"));
  const path = join(directory, "refresh-token.enc");
  if (stored && !userData) writeFileSync(path, `encrypted:${stored}`, { mode: 0o600 });
  const calls = { browser: [], fetch: [], windows: [], dialogs: [], timers: [], cleanup: [], quitEvents: [], errorBoxes: [], partitions: [] };
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
      this.webContents = new EventEmitter();
      this.webContents.setWindowOpenHandler = () => {};
      calls.windows.push(this);
    }
    loadURL(url) { this.url = url; return Promise.resolve(); }
    show() {} focus() {} isMinimized() { return false; }
    destroy() { this.destroyed = true; this.emit("closed"); }
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
    app, BrowserWindow, removeSavedToken,
    safeStorage: {
      isEncryptionAvailable: () => available,
      getSelectedStorageBackend: () => "gnome_libsecret",
      encryptString: (value) => Buffer.from(`encrypted:${value}`),
      decryptString: (bytes) => bytes.toString().slice(10),
      ...safeStorage,
    },
    dialog: { showErrorBox: (title, content) => calls.errorBoxes.push({ title, content }), showMessageBox: async (options) => {
      calls.dialogs.push(options);
      return showMessageBox ? showMessageBox(options) : { response: calls.dialogResponse ?? 1 };
    } },
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
      webRequest: { onBeforeSendHeaders: (_filter, handler) => { calls.headers = handler; } },
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
      if (specifier === "./token-store.mjs" && context.parentURL === new URL(`../src/main.mjs?test=${id}`, import.meta.url).href) {
        const resolved = nextResolve(specifier, context);
        return { ...resolved, url: `${resolved.url}?test=${id}` };
      }
      if (specifier === "electron") return { url: `stub:electron-${id}`, shortCircuit: true };
      return nextResolve(specifier, context);
    },
    load(url, context, nextLoad) {
      if (url === `stub:electron-${id}`) return {
        format: "module", shortCircuit: true,
        source: `export const { app, BrowserWindow, safeStorage, dialog, Menu, net, session, shell } = globalThis.${key};`,
      };
      const loaded = nextLoad(url, context);
      if (url === new URL(`../src/main.mjs?test=${id}`, import.meta.url).href) {
        return { ...loaded, source: `${loaded.source}\nexport { refreshTokens as testRefreshTokens };` };
      }
      if (removeSavedToken && url === new URL(`../src/token-store.mjs?test=${id}`, import.meta.url).href) {
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
  const { testRefreshTokens } = await import(`../src/main.mjs?test=${id}`);
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
  return { app, calls, defaultState, tokenState, path, header, signOut, callback, refreshTokens: testRefreshTokens, safeStorage: fixture.safeStorage };
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


test("a second launch uses the last rotated token without browser sign-in", async (t) => {
  const first = await launch(t);
  const second = await launch(t, { userData: dirname(first.path), fetch: () => response(tokens("second-launch")) });
  assert.equal(second.calls.fetch[0].body.get("refresh_token"), "fixture-refresh-rotated");
  assert.deepEqual(second.calls.browser, []);
  assert.equal(second.calls.windows.length, 1);
  assert.equal(readFileSync(first.path, "utf8"), "encrypted:fixture-refresh-second-launch");
});

test("activation during authorization-code exchange cannot start another sign-in", async (t) => {
  let resolveExchange;
  const pending = new Promise((resolve) => { resolveExchange = resolve; });
  const { app, calls, callback } = await launch(t, { stored: null, fetch: () => pending });
  callback();
  await flush();
  app.emit("activate");
  await flush();
  assert.equal(calls.browser.length, 1);
  assert.equal(calls.fetch.length, 1);
  resolveExchange(response(tokens()));
  await flush();
  assert.equal(calls.windows.length, 1);
});

test("browser-open failure can retry with a fresh authorization request", async (t) => {
  const { calls, app } = await launch(t, {
    stored: null,
    openExternal: (count) => { if (count === 1) throw new Error("fixture browser unavailable"); },
  });
  // Quit was selected in the error dialog; emulate a later activation instead.
  app.emit("activate");
  await flush();
  assert.equal(calls.browser.length, 2);
  assert.notEqual(new URL(calls.browser[0]).searchParams.get("state"), new URL(calls.browser[1]).searchParams.get("state"));
});

test("a scheduled refresh rotates the stored token and sends the latest token", async (t) => {
  const { calls, path, header } = await launch(t, { fetch: (count) => response(tokens(`rotation-${count}`)) });
  t.mock.timers.tick(840_000);
  await flush();
  assert.equal(calls.fetch[1].body.get("refresh_token"), "fixture-refresh-rotation-1");
  assert.equal(readFileSync(path, "utf8"), "encrypted:fixture-refresh-rotation-2");
  assert.equal(header(), "Bearer fixture-access-rotation-2");
});

test("scheduled invalid_grant clears the old account window before reauthentication", async (t) => {
  const { calls, path, header } = await launch(t, {
    fetch: (count) => count === 1 ? response(tokens()) : response({ error: "invalid_grant" }, false),
  });
  t.mock.timers.tick(840_000);
  await flush();
  assert.equal(existsSync(path), false);
  assert.equal(header(), undefined);
  assert.equal(calls.windows[0].destroyed, true);
  assert.equal(calls.browser.length, 1);
});

test("an encryption failure falls back to memory and discards the stale rotated token", async (t) => {
  const logs = [];
  t.mock.method(console, "error", (...args) => logs.push(args));
  const { calls, path, header } = await launch(t, {
    safeStorage: { encryptString: () => { throw new Error("fixture-secret-must-not-be-logged"); } },
  });
  assert.equal(calls.windows.length, 1);
  assert.equal(existsSync(path), false);
  assert.equal(header(), "Bearer fixture-access-rotated");
  assert.equal(logs.length, 1);
  assert.equal(JSON.stringify(logs).includes("fixture-secret"), false);
});


function deferred() {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
}

test("sign-out retires renderer then awaits storage and HTTP cache cleanup before quit", async (t) => {
  const storage = deferred();
  const cache = deferred();
  const { app, calls, signOut, path, header } = await launch(t, {
    clearStorageData: () => storage.promise,
    clearCache: () => cache.promise,
  });
  const signingOut = signOut().click();
  await flush();
  assert.equal(calls.windows[0].destroyed, true);
  assert.equal(existsSync(path), false);
  assert.equal(header(), undefined);
  assert.deepEqual(calls.cleanup, ["storage"]);
  assert.equal(calls.quit, undefined);
  // Window shutdown must not quit the process while cleanup is still pending.
  app.emit("window-all-closed");
  app.emit("activate");
  assert.equal(calls.quit, undefined);
  storage.resolve();
  await flush();
  assert.deepEqual(calls.cleanup, ["storage", "cache"]);
  assert.equal(calls.quit, undefined);
  cache.resolve();
  await signingOut;
  assert.equal(calls.quit, 1);
});

for (const operation of ["storage", "cache"]) {
  test(`sign-out reports ${operation} cleanup failure and attempts both cleanup operations`, async (t) => {
    const failure = async () => { throw new Error("fixture-sensitive-cleanup-error"); };
    const { calls, signOut } = await launch(t, {
      clearStorageData: operation === "storage" ? failure : undefined,
      clearCache: operation === "cache" ? failure : undefined,
    });
    await signOut().click();
    assert.equal(calls.windows[0].destroyed, true);
    assert.deepEqual(calls.cleanup, ["storage", "cache"]);
    assert.equal(calls.dialogs.length, 1);
    assert.match(calls.dialogs[0].message, /could not.*(?:storage|cache|data)/i);
    assert.equal(JSON.stringify(calls.dialogs).includes("fixture-sensitive"), false);
    assert.equal(calls.quit, 1);
  });
}

for (const [name, storageOptions] of [
  ["unavailable encryption", { available: false }],
  ["basic_text backend", { available: true, safeStorage: { getSelectedStorageBackend: () => "basic_text" } }],
  ["encryption failure", { safeStorage: { encryptString: () => { throw new Error("fixture encryption failure"); } } }],
]) {
  test(`${name} successful account B sign-in prevents account A restoration`, async (t) => {
    t.mock.method(console, "error", () => {});
    const first = await launch(t, {
      stored: "fixture-account-A", available: false,
      ...storageOptions, fetch: () => response(tokens("account-B")),
    });
    assert.equal(first.calls.browser.length, 1);
    if (name === "encryption failure") first.safeStorage.isEncryptionAvailable = () => true;
    first.callback();
    await flush();
    assert.equal(first.header(), "Bearer fixture-access-account-B");
    assert.equal(first.calls.windows.length, 1);
    assert.equal(existsSync(first.path), false, "previous account credential removed");
    const next = await launch(t, { userData: dirname(first.path) });
    assert.equal(next.calls.fetch.length, 0, "secure relaunch cannot refresh account A");
    assert.equal(next.calls.browser.length, 1);
    assert.equal(next.calls.windows.length, 0);
  });
}

for (const [name, storageOptions] of [
  ["unavailable encryption", { available: false }],
  ["basic_text backend", { available: true, safeStorage: { getSelectedStorageBackend: () => "basic_text" } }],
  ["encryption failure", { safeStorage: { encryptString: () => { throw new Error("fixture encryption failure"); } } }],
]) {
  test(`${name} reports previous credential deletion failure before opening account B`, async (t) => {
    t.mock.method(console, "error", () => {});
    const { calls, path, callback, header, safeStorage } = await launch(t, {
      stored: "fixture-account-A", available: false,
      ...storageOptions, fetch: () => response(tokens("account-B")),
      removeSavedToken: () => { throw new Error("fixture-secret-delete-failure"); },
    });
    if (name === "encryption failure") safeStorage.isEncryptionAvailable = () => true;
    callback();
    await flush();
    assert.equal(calls.dialogs.length, 1);
    assert.match(calls.dialogs[0].detail, /could not remove.*saved sign-in/i);
    assert.equal(JSON.stringify(calls.dialogs).includes("fixture-secret"), false);
    assert.equal(readFileSync(path, "utf8"), "encrypted:fixture-account-A");
    assert.equal(header(), undefined);
    assert.equal(calls.windows.length, 0);
  });
}

test("overlapping refresh requests share one exchange and the next refresh uses its replacement", async (t) => {
  const pending = deferred();
  const { calls, path, refreshTokens } = await launch(t, {
    fetch: (count) => count === 2 ? pending.promise : response(tokens(`overlap-${count}`)),
  });
  const first = refreshTokens();
  const overlapping = refreshTokens();
  await flush();
  assert.equal(calls.fetch.length, 2, "one startup refresh plus one shared pending refresh");
  assert.equal(calls.fetch[1].body.get("refresh_token"), "fixture-refresh-overlap-1");
  pending.resolve(response(tokens("overlap-2")));
  assert.deepEqual(await Promise.all([first, overlapping]), [true, true]);
  assert.equal(readFileSync(path, "utf8"), "encrypted:fixture-refresh-overlap-2");
  await refreshTokens();
  assert.equal(calls.fetch.length, 3);
  assert.equal(calls.fetch[2].body.get("refresh_token"), "fixture-refresh-overlap-2");
});


test("an existing authentication dialog cannot quit during sign-out cleanup", async (t) => {
  const dialog = deferred();
  const storage = deferred();
  const { calls, signOut } = await launch(t, {
    fetch: (count) => {
      if (count === 2) throw new Error("fixture offline");
      return response(tokens());
    },
    showMessageBox: () => dialog.promise,
    clearStorageData: () => storage.promise,
  });
  t.mock.timers.tick(840_000);
  await flush();
  assert.equal(calls.dialogs.length, 1);
  const signingOut = signOut().click();
  await flush();
  dialog.resolve({ response: 1 });
  await flush();
  assert.equal(calls.quit, undefined, "old dialog cannot bypass awaited session cleanup");
  storage.resolve();
  await signingOut;
  assert.equal(calls.quit, 1);
});


for (const operation of ["storage", "cache"]) {
  test(`native Quit during ${operation} cleanup waits and final Quit is allowed`, async (t) => {
    const pending = deferred();
    const { app, calls, signOut } = await launch(t, {
      clearStorageData: operation === "storage" ? () => pending.promise : undefined,
      clearCache: operation === "cache" ? () => pending.promise : undefined,
    });
    const cleanup = signOut().click();
    await flush();
    app.quit();
    app.quit();
    assert.deepEqual(calls.quitEvents, [true, true]);
    assert.equal(calls.quit, undefined);
    pending.resolve();
    await cleanup;
    assert.deepEqual(calls.quitEvents, [true, true, false]);
    assert.equal(calls.quit, 1);
    // Cleanup first, then native Quit: no permanent lifecycle veto remains.
    app.quit();
    assert.equal(calls.quitEvents.at(-1), false);
    assert.equal(calls.quit, 2);
  });
}

test("ordinary native Quit without Sign Out is allowed", async (t) => {
  const { app, calls } = await launch(t);
  app.quit();
  assert.deepEqual(calls.quitEvents, [false]);
  assert.equal(calls.quit, 1);
});

test("native Quit waits for Sign Out failure reporting", async (t) => {
  const report = deferred();
  const { app, calls, signOut } = await launch(t, {
    clearStorageData: () => { throw new Error("fixture-private-failure"); },
    showMessageBox: () => report.promise,
  });
  const cleanup = signOut().click();
  await flush();
  assert.equal(calls.dialogs.length, 1);
  app.quit();
  assert.deepEqual(calls.quitEvents, [true]);
  assert.equal(calls.quit, undefined);
  report.resolve({ response: 0 });
  await cleanup;
  assert.equal(calls.quit, 1);
});

for (const operation of ["storage", "cache"]) {
  test(`hung ${operation} cleanup is bounded, reports failure, and permits quit`, async (t) => {
    const pending = deferred();
    const { app, calls, signOut } = await launch(t, {
      clearStorageData: operation === "storage" ? () => pending.promise : undefined,
      clearCache: operation === "cache" ? () => pending.promise : undefined,
    });
    const cleanup = signOut().click();
    await flush();
    app.quit();
    assert.equal(calls.quitEvents.at(-1), true);
    t.mock.timers.tick(5_000);
    await flush();
    assert.deepEqual(calls.cleanup, ["storage", "cache"]);
    assert.equal(calls.dialogs.length, 1);
    assert.match(calls.dialogs[0].message, /could not clear/);
    assert.equal(calls.quit, 1);
    await cleanup;
    pending.resolve();
    await flush();
    assert.equal(calls.quit, 1, "late completion does not quit twice");
  });
}

for (const mode of ["pending", "rejected"]) {
  test(`${mode} failure dialog cannot hold Sign Out forever`, async (t) => {
    const { app, calls, signOut } = await launch(t, {
      clearCache: () => { throw new Error("fixture-private-failure"); },
      showMessageBox: () => mode === "pending" ? new Promise(() => {}) : Promise.reject(new Error("fixture-dialog-failure")),
    });
    const cleanup = signOut().click();
    await flush();
    if (mode === "pending") {
      app.quit();
      assert.equal(calls.quitEvents.at(-1), true);
      t.mock.timers.tick(5_000);
      await flush();
    }
    assert.equal(calls.errorBoxes.length, 1, "fallback surfaces cleanup failure");
    assert.equal(JSON.stringify(calls.errorBoxes).includes("fixture-private"), false);
    assert.equal(calls.quit, 1);
    await cleanup;
  });
}

for (const grant of ["refresh", "code"]) {
  test(`Sign Out aborts ${grant} exchange and late response cannot repopulate default session`, async (t) => {
    const late = deferred();
    const cache = deferred();
    const { calls, callback, signOut, path, header, defaultState, tokenState } = await launch(t, {
      stored: grant === "code" ? null : "fixture-refresh-saved",
      fetch: (count) => grant === "refresh" && count === 1 ? response(tokens()) : late.promise,
      clearCache: () => cache.promise,
    });
    if (grant === "code") callback();
    else t.mock.timers.tick(840_000);
    await flush();
    const request = calls.fetch.at(-1);
    const cleanup = signOut().click();
    await flush();
    assert.equal(defaultState.cookie, null);
    // Ignore abort deliberately: isolation must protect storage even if a response won the race.
    late.resolve({ ...response(tokens("late")), setCookie: "late-cookie", cachedContent: "late-account-content" });
    await flush();
    assert.equal(defaultState.cookie, null);
    assert.equal(request.options.signal?.aborted, true, "abort before session cleanup");
    assert.equal(defaultState.cache, "account-content", "late response cannot rewrite existing default cache");
    assert.equal(tokenState.cookie, null, "token fetch also omits response cookies");
    assert.equal(tokenState.cache, null, "token fetch does not cache account content");
    assert.equal(existsSync(path), false);
    assert.equal(header(), undefined);
    assert.equal(calls.dialogs.length, 0);
    cache.resolve();
    await cleanup;
    assert.deepEqual(defaultState, { cookie: null, cache: null });
  });
}

test("every token request uses an isolated nonpersistent session without cookies or caching", async (t) => {
  const { calls, defaultState, tokenState } = await launch(t);
  assert.equal(calls.partitions.length, 1);
  assert.notEqual(calls.fetch[0].state, defaultState);
  assert.equal(calls.fetch[0].state, tokenState);
  assert.equal(calls.fetch[0].options.credentials, "omit");
  assert.equal(calls.fetch[0].options.cache, "no-store");
  assert.ok(calls.fetch[0].options.signal instanceof AbortSignal);
});


test("Sign Out aborts outstanding token requests before deleting storage", async (t) => {
  const pending = deferred();
  let signal;
  const { calls, signOut } = await launch(t, {
    fetch: (count, options) => {
      if (count === 1) return response(tokens());
      signal = options.signal;
      return pending.promise;
    },
    clearStorageData: () => assert.equal(signal?.aborted, true),
  });
  t.mock.timers.tick(840_000);
  await flush();
  await signOut().click();
  assert.equal(signal?.aborted, true);
  pending.resolve(response(tokens("late")));
  await flush();
  assert.equal(calls.dialogs.length, 0);
});
