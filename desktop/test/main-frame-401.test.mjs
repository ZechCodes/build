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
  const { calls, beginAuthentication } = await launch(t);
  // Model an already pending native login whose credentials have been retired.
  await beginAuthentication();
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
