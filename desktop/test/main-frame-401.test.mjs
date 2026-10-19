import assert from "node:assert/strict";
import test from "node:test";
import { APP_URL } from "../src/security-policy.mjs";
import { flush, launch, response, tokens } from "./helpers/main-auth-harness.mjs";

const deferred = () => {
  let resolve;
  const promise = new Promise((complete) => { resolve = complete; });
  return { promise, resolve };
};
const loads = (window) => window.actions.filter((action) => action === "load").length;
function unauthorized(calls, window, overrides = {}) {
  let result;
  assert.equal(typeof calls.responseHeaders, "function", "401 responses must be intercepted before rendering");
  calls.responseHeaders({
    id: 1, url: APP_URL, statusCode: 401,
    resourceType: "mainFrame", webContentsId: window.webContents.id,
    ...overrides,
  }, (value) => { result = value; });
  return result;
}
function navigate(window, url = APP_URL, status = 200) {
  window.webContents.emit("did-navigate", {}, url, status, "fixture status");
}
function startNavigation(window, url = APP_URL) {
  window.webContents.emit("did-start-navigation", {}, url, false, true);
}

test("launch 401 cancels rendering, refreshes once and reloads the authenticated desktop", async (t) => {
  const { calls, header } = await launch(t, { fetch: (count) => response(tokens(`rotation-${count}`)) });
  const window = calls.windows[0];
  assert.deepEqual(unauthorized(calls, window), { cancel: true });
  window.emit("ready-to-show");
  await flush();
  assert.equal(calls.fetch.length, 2);
  assert.equal(calls.fetch[1].body.get("grant_type"), "refresh_token");
  assert.equal(calls.fetch[1].body.get("refresh_token"), "fixture-refresh-rotation-1");
  assert.equal(header(), "Bearer fixture-access-rotation-2");
  assert.equal(loads(window), 2);
  assert.equal(window.url, APP_URL);
  assert.equal(window.actions.includes("show"), false, "rejected document stays hidden");
  assert.deepEqual(calls.browser, []);
  navigate(window);
  assert.equal(window.actions.at(-1), "show");
});

for (const [name, failedRefresh] of [
  ["invalid grant", () => response({ error: "invalid_grant" }, false)],
  ["server error", () => response({ error: "server_error" }, false)],
  ["network error", () => { throw new Error("fixture unavailable"); }],
  ["incomplete tokens", () => response({ access_token: "fixture incomplete" })],
]) {
  test(`401 ${name} refresh falls back to native PKCE without a refresh error dialog`, async (t) => {
    const { calls, callback, header } = await launch(t, {
      fetch: (count) => count === 2 ? failedRefresh() : response(tokens(`attempt-${count}`)),
    });
    const window = calls.windows[0];
    unauthorized(calls, window);
    await flush();
    assert.equal(calls.browser.length, 1);
    assert.equal(new URL(calls.browser[0]).pathname, "/oauth/authorize");
    assert.equal(header(), undefined);
    assert.deepEqual(calls.dialogs, []);
    callback();
    await flush();
    assert.equal(calls.fetch[2].body.get("grant_type"), "authorization_code");
    assert.ok(calls.fetch[2].body.get("code_verifier"));
    assert.equal(window.url, APP_URL);
    assert.equal(loads(window), 2);
    navigate(window);
    assert.equal(window.actions.at(-1), "show");
  });
}

test("without an in-memory refresh token 401 starts native authentication immediately", async (t) => {
  const { calls, clearCredentials } = await launch(t);
  clearCredentials();
  const window = calls.windows[0];
  unauthorized(calls, window);
  await flush();
  assert.equal(calls.fetch.length, 1);
  assert.equal(calls.browser.length, 1);
  assert.deepEqual(calls.dialogs, []);
});

test("duplicate headers and navigation notifications coalesce while refresh is pending", async (t) => {
  const pending = deferred();
  const { app, calls } = await launch(t, { fetch: (count) => count === 2 ? pending.promise : response(tokens()) });
  const window = calls.windows[0];
  unauthorized(calls, window);
  unauthorized(calls, window);
  navigate(window, APP_URL, 401);
  window.emit("ready-to-show");
  app.emit("activate");
  app.emit("second-instance", {}, ["build"]);
  await flush();
  assert.equal(calls.fetch.length, 2);
  assert.equal(window.actions.includes("show"), false);
  assert.deepEqual(calls.dialogs, []);
  pending.resolve(response(tokens("recovered")));
  await flush();
  unauthorized(calls, window); // Duplicate canceled request, after retry started.
  navigate(window, "https://getbuild.ing/auth/login", 200);
  assert.deepEqual(calls.dialogs, []);
  assert.deepEqual(calls.browser, []);
  navigate(window);
  assert.equal(window.actions.at(-1), "show");
});

test("401 on refreshed retry escalates once to OAuth; post-OAuth rejection stops with native error", async (t) => {
  const { calls, callback } = await launch(t);
  const window = calls.windows[0];
  unauthorized(calls, window);
  await flush();
  unauthorized(calls, window, { id: 2 });
  await flush();
  assert.equal(calls.browser.length, 1);
  unauthorized(calls, window, { id: 2 });
  navigate(window, APP_URL, 401);
  await flush();
  assert.deepEqual(calls.dialogs, []);
  callback();
  await flush();
  unauthorized(calls, window, { id: 3 });
  unauthorized(calls, window, { id: 3 });
  navigate(window, APP_URL, 401);
  await flush();
  assert.equal(calls.fetch.length, 3, "startup, one recovery refresh, one code exchange");
  assert.equal(calls.browser.length, 1);
  assert.equal(calls.dialogs.length, 1);
  assert.equal(calls.dialogs[0].title, "Build sign-in failed");
  assert.deepEqual(calls.dialogs[0].buttons, ["Try Again", "Quit"]);
  assert.equal(calls.quit, 1);
});

test("OAuth failure uses the existing native Try Again/Quit dialog", async (t) => {
  const { calls, callback } = await launch(t, {
    fetch: (count) => count === 1 ? response(tokens()) : response({ error: "invalid_grant" }, false),
  });
  unauthorized(calls, calls.windows[0]);
  await flush();
  callback();
  await flush();
  assert.equal(calls.dialogs.length, 1);
  assert.equal(calls.dialogs[0].title, "Build sign-in failed");
  assert.deepEqual(calls.dialogs[0].buttons, ["Try Again", "Quit"]);
  assert.equal(calls.browser.length, 1);
});

test("a 401 navigation commit recovers when response interception was unavailable", async (t) => {
  const { calls } = await launch(t);
  const window = calls.windows[0];
  navigate(window, APP_URL, 401);
  await flush();
  assert.equal(calls.fetch.length, 2);
  assert.equal(loads(window), 2);
  navigate(window);
  startNavigation(window); // A later ordinary reload gets a fresh recovery budget.
  navigate(window, APP_URL, 401);
  await flush();
  assert.equal(calls.fetch.length, 3);
  assert.equal(loads(window), 3);
});

test("scheduled refresh already in flight is shared and 401 takes over failure policy", async (t) => {
  const pending = deferred();
  const { calls } = await launch(t, { fetch: (count) => count === 2 ? pending.promise : response(tokens()) });
  t.mock.timers.tick(840_000);
  await flush();
  unauthorized(calls, calls.windows[0]);
  await flush();
  assert.equal(calls.fetch.length, 2);
  pending.resolve(response({ error: "invalid_grant" }, false));
  await flush();
  assert.equal(calls.browser.length, 1);
  assert.deepEqual(calls.dialogs, []);
});

for (const url of ["https://getbuild.ing/app", "https://getbuild.ing/app/", "https://getbuild.ing/auth/login?next=/app/&view=signin", "https://getbuild.ing/auth/passkey"]) {
  test(`trusted main-frame 401 at ${new URL(url).pathname} recovers`, async (t) => {
    const { calls } = await launch(t);
    assert.deepEqual(unauthorized(calls, calls.windows[0], { url }), { cancel: true });
    await flush();
    assert.equal(calls.fetch.length, 2);
    assert.equal(calls.windows[0].url, APP_URL);
    assert.deepEqual(calls.browser, []);
  });
}

const unrelated = [
  { resourceType: "xhr", url: "https://getbuild.ing/api/devices" },
  { resourceType: "subFrame" }, { resourceType: "script" },
  { webContentsId: 999 }, { webContentsId: undefined },
  { url: "https://example.com/app/desktop" },
  { url: "https://getbuild.ing.evil.test/app/desktop" },
  { url: "http://getbuild.ing/app/desktop" },
  { url: "https://getbuild.ing:8443/app/desktop" },
  { url: "https://getbuild.ing/app/desktop/" },
  { url: "https://getbuild.ing/app/downloads" },
  { url: "https://getbuild.ing/%61pp/desktop" },
  { url: "https://getbuild.ing/auth-other/login" },
  { url: "https://getbuild.ing/" }, { url: "not a url" },
  { statusCode: 403 }, { statusCode: 500 },
];
test("response interception ignores unrelated resources, windows, URLs and statuses", async (t) => {
  const { calls } = await launch(t);
  assert.deepEqual(calls.responseFilter, { urls: ["https://getbuild.ing/*"] });
  const window = calls.windows[0];
  for (const details of unrelated) assert.deepEqual(unauthorized(calls, window, details), {}, JSON.stringify(details));
  await flush();
  assert.equal(calls.fetch.length, 1);
  assert.deepEqual(calls.browser, []);
  assert.deepEqual(window.actions, ["load"]);
});

test("fallback ignores unrelated URLs and stale windows, including late success", async (t) => {
  const { app, calls } = await launch(t);
  const retired = calls.windows[0];
  retired.destroy();
  app.emit("activate");
  const current = calls.windows[1];
  navigate(retired, APP_URL, 401);
  for (const { url } of unrelated.filter((details) => details.url)) navigate(current, url, 401);
  await flush();
  assert.equal(calls.fetch.length, 1);
  unauthorized(calls, current);
  await flush();
  navigate(retired);
  navigate(current, "https://getbuild.ing/app/", 200);
  assert.equal(current.actions.includes("show"), false);
  navigate(current);
  assert.equal(current.actions.at(-1), "show");
});

test("sign-out retires a pending recovery and its late success cannot reload a window", async (t) => {
  const pending = deferred();
  const { calls, signOut, header } = await launch(t, { fetch: (count) => count === 2 ? pending.promise : response(tokens()) });
  const window = calls.windows[0];
  unauthorized(calls, window);
  await flush();
  await signOut().click();
  pending.resolve(response(tokens("late")));
  await flush();
  assert.equal(header(), undefined);
  assert.equal(loads(window), 1);
  assert.deepEqual(calls.browser, []);
  assert.deepEqual(calls.dialogs, []);
});


test("Try Again after a rejected OAuth retry starts one explicit new OAuth attempt", async (t) => {
  const dialog = deferred();
  let dialogs = 0;
  const { calls, callback } = await launch(t, {
    showMessageBox: () => ++dialogs === 1 ? dialog.promise : new Promise(() => {}),
  });
  const window = calls.windows[0];
  unauthorized(calls, window);
  await flush();
  unauthorized(calls, window, { id: 2 });
  await flush();
  callback();
  await flush();
  unauthorized(calls, window, { id: 3 });
  await flush();
  assert.equal(calls.dialogs.length, 1);
  dialog.resolve({ response: 0 });
  await flush();
  assert.equal(calls.browser.length, 2, "only the explicit native retry launches OAuth again");
  assert.equal(calls.fetch.length, 3, "retry does not reuse another refresh chain");
  callback();
  await flush();
  unauthorized(calls, window, { id: 4 });
  await flush();
  assert.equal(calls.dialogs.length, 2);
  assert.equal(calls.browser.length, 2, "a subsequent rejection waits for another explicit choice");
  assert.equal(calls.fetch.length, 4);
});

test("scheduled transient refresh failure belongs to 401 recovery without an extra dialog", async (t) => {
  let reject;
  const pending = new Promise((_resolve, refuse) => { reject = refuse; });
  const { calls } = await launch(t, { fetch: (count) => count === 2 ? pending : response(tokens()) });
  t.mock.timers.tick(840_000);
  await flush();
  unauthorized(calls, calls.windows[0]);
  reject(new Error("fixture transient outage"));
  await flush();
  assert.equal(calls.browser.length, 1);
  assert.deepEqual(calls.dialogs, []);
  assert.equal(calls.fetch.length, 2);
});

test("only an explicit successful document status can finish recovery", async (t) => {
  const { calls } = await launch(t);
  const window = calls.windows[0];
  unauthorized(calls, window);
  await flush();
  for (const status of [undefined, 0, 199, 302]) {
    window.webContents.emit("did-navigate", {}, APP_URL, status);
    assert.equal(window.actions.includes("show"), false, `status ${status}`);
  }
  navigate(window);
  assert.equal(window.actions.at(-1), "show");
});

test("an older scheduled-refresh dialog cannot restore saved credentials during native recovery", async (t) => {
  const dialog = deferred();
  const { calls } = await launch(t, {
    fetch: (count) => count === 1 ? response(tokens()) : response({ error: "server_error" }, false),
    showMessageBox: () => dialog.promise,
  });
  t.mock.timers.tick(840_000);
  await flush();
  assert.equal(calls.dialogs.length, 1);
  unauthorized(calls, calls.windows[0]);
  await flush();
  assert.equal(calls.fetch.length, 3);
  assert.equal(calls.browser.length, 1);
  dialog.resolve({ response: 0 });
  await flush();
  assert.equal(calls.fetch.length, 3, "older retry cannot restore rejected saved credentials");
  assert.equal(calls.dialogs.length, 1);
});

for (const succeeds of [true, false]) {
  test(`closing a recovery window releases activation after late refresh ${succeeds ? "success" : "failure"}`, async (t) => {
    const pending = deferred();
    const { app, calls } = await launch(t, {
      fetch: (count) => count === 2 ? pending.promise : response(tokens()),
    });
    const retired = calls.windows[0];
    unauthorized(calls, retired);
    await flush();
    retired.destroy();
    pending.resolve(succeeds ? response(tokens("late")) : response({ error: "server_error" }, false));
    await flush();
    assert.equal(calls.windows.length, 1, "late completion never reopens the retired window");
    assert.deepEqual(calls.browser, []);
    assert.deepEqual(calls.dialogs, []);
    app.emit("activate");
    await flush();
    assert.equal(calls.windows.length, 2, "a later explicit activation is not wedged by old recovery");
    retired.emit("ready-to-show");
    assert.equal(retired.actions.includes("show"), false, "retired ready-to-show cannot reveal its renderer");
    navigate(retired);
    assert.equal(calls.windows[1].actions.includes("show"), false);
  });
}


for (const status of [403, 500]) {
  test(`replacement document status ${status} uses a native failure dialog`, async (t) => {
    const { app, calls } = await launch(t);
    const window = calls.windows[0];
    unauthorized(calls, window);
    await flush();
    navigate(window, APP_URL, status);
    await flush();
    assert.equal(calls.dialogs.length, 1);
    assert.equal(calls.dialogs[0].title, "Build sign-in failed");
    assert.equal(window.actions.includes("show"), false);
    app.emit("second-instance", {}, ["build"]);
    assert.equal(window.actions.includes("show"), false);
    assert.equal(calls.quit, 1);
  });
}

for (const signal of ["loadURL", "did-fail-load"]) {
  test(`replacement document ${signal} failure uses a native failure dialog`, async (t) => {
    const { calls } = await launch(t, {
      loadURL: (window) => loads(window) > 1 && signal === "loadURL"
        ? Promise.reject(Object.assign(new Error("fixture load failure"), { code: "ERR_CONNECTION_FAILED", errno: -2 }))
        : Promise.resolve(),
    });
    const window = calls.windows[0];
    unauthorized(calls, window);
    await flush();
    if (signal === "did-fail-load") window.webContents.emit("did-fail-load", {}, -2, "fixture failure", APP_URL, true);
    await flush();
    assert.equal(calls.dialogs.length, 1);
    assert.equal(calls.dialogs[0].title, "Build sign-in failed");
    assert.equal(window.actions.includes("show"), false);
  });
}

test("intentional canceled 401 loads and subframe failures cannot fail a pending recovery", async (t) => {
  const { calls } = await launch(t, {
    loadURL: (window) => loads(window) > 1
      ? Promise.reject(Object.assign(new Error("fixture canceled"), { code: "ERR_ABORTED", errno: -3 }))
      : Promise.resolve(),
  });
  const window = calls.windows[0];
  unauthorized(calls, window);
  await flush();
  window.webContents.emit("did-fail-load", {}, -3, "fixture canceled", APP_URL, true);
  window.webContents.emit("did-fail-load", {}, -2, "fixture frame failure", APP_URL, false);
  await flush();
  assert.deepEqual(calls.dialogs, []);
  navigate(window);
  assert.equal(window.actions.at(-1), "show");
});

test("an older refresh retry already awaiting the shared exchange cannot duplicate recovery loads", async (t) => {
  const dialog = deferred();
  const pending = deferred();
  const { calls } = await launch(t, {
    fetch: (count) => {
      if (count === 2) return response({ error: "server_error" }, false);
      if (count === 3) return pending.promise;
      return response(tokens());
    },
    showMessageBox: () => dialog.promise,
  });
  t.mock.timers.tick(840_000);
  await flush();
  dialog.resolve({ response: 0 });
  await flush();
  assert.equal(calls.fetch.length, 3);
  const window = calls.windows[0];
  unauthorized(calls, window);
  pending.resolve(response(tokens("recovered")));
  await flush();
  assert.equal(loads(window), 2, "only recovery may reload the shared refresh result");
  unauthorized(calls, window, { id: 2 });
  await flush();
  assert.equal(calls.browser.length, 1, "a rejected refresh retry still has one native OAuth step");
  assert.equal(calls.dialogs.length, 1, "the only dialog was the earlier scheduled error");
});

test("a canceled header response may fail with ERR_BLOCKED_BY_CLIENT after retry starts", async (t) => {
  let reject;
  const canceled = new Promise((_resolve, refuse) => { reject = refuse; });
  const errors = [];
  t.mock.method(console, "error", (...args) => errors.push(args));
  const { calls } = await launch(t, {
    loadURL: (window) => loads(window) === 1 ? canceled : Promise.resolve(),
  });
  const window = calls.windows[0];
  unauthorized(calls, window);
  await flush();
  reject(Object.assign(new Error("fixture canceled headers"), { code: "ERR_BLOCKED_BY_CLIENT", errno: -20 }));
  window.webContents.emit("did-fail-load", {}, -20, "ERR_BLOCKED_BY_CLIENT", APP_URL, true);
  await flush();
  assert.deepEqual(calls.dialogs, []);
  assert.deepEqual(errors, [], "intentional cancellation is not logged as a load failure");
  navigate(window);
  assert.equal(window.actions.at(-1), "show");
});

test("a retired replacement-load promise cannot fail the newer OAuth retry", async (t) => {
  let reject;
  const pending = new Promise((_resolve, refuse) => { reject = refuse; });
  const { calls, callback } = await launch(t, {
    loadURL: (window) => loads(window) === 2 ? pending : Promise.resolve(),
  });
  const window = calls.windows[0];
  unauthorized(calls, window);
  await flush();
  unauthorized(calls, window, { id: 2 });
  await flush();
  callback();
  await flush();
  reject(Object.assign(new Error("fixture retired load"), { code: "ERR_CONNECTION_FAILED", errno: -2 }));
  await flush();
  assert.deepEqual(calls.dialogs, []);
  navigate(window);
  assert.equal(window.actions.at(-1), "show");
});

test("a genuinely blocked replacement load still reports failure after an earlier canceled 401", async (t) => {
  const { calls } = await launch(t, {
    loadURL: (window) => loads(window) > 1
      ? Promise.reject(Object.assign(new Error("fixture blocked retry"), { code: "ERR_BLOCKED_BY_CLIENT", errno: -20 }))
      : Promise.resolve(),
  });
  unauthorized(calls, calls.windows[0]);
  await flush();
  assert.equal(calls.dialogs.length, 1, "cancellation belongs to the rejected load, not every future load at its URL");
});
