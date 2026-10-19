import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { APP_URL } from "../src/security-policy.mjs";
import { flush, launch, response, tokens } from "./helpers/main-auth-harness.mjs";

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const loads = (window) => window.actions.filter((action) => action === "load").length;
const shown = (window) => window.actions.filter((action) => action === "show").length;
function unauthorized(calls, window, id) {
  calls.responseHeaders({
    id, url: APP_URL, statusCode: 401, resourceType: "mainFrame", webContentsId: window.webContents.id,
  }, (result) => assert.deepEqual(result, { cancel: true }));
}
function commit(window, status = 200) {
  window.webContents.emit("did-navigate", {}, APP_URL, status, "fixture status");
}
function finish(window) { window.webContents.emit("did-finish-load"); }
function bodyFailure(window) {
  window.webContents.emit("did-fail-load", {}, -354, "ERR_INCOMPLETE_CHUNKED_ENCODING", APP_URL, true);
}
const loadError = () => Object.assign(new Error("fixture incomplete body"), {
  code: "ERR_INCOMPLETE_CHUNKED_ENCODING", errno: -354,
});
async function retrying(t, phase, options = {}) {
  const fixture = await launch(t, { fetch: (count) => response(tokens(`step-${count}`)), ...options });
  const window = fixture.calls.windows[0];
  unauthorized(fixture.calls, window, 101);
  await flush();
  if (phase === "oauth") {
    unauthorized(fixture.calls, window, 102);
    await flush();
    fixture.callback();
    await flush();
  }
  return { ...fixture, window };
}

for (const phase of ["refresh", "oauth"]) {
  for (const status of [404, 503]) {
    test(`post-${phase} HTTP ${status} keeps credentials and Try Again reloads without authentication`, async (t) => {
      const dialog = deferred();
      const { calls, window, header, path } = await retrying(t, phase, { showMessageBox: () => dialog.promise });
      const credential = header();
      const saved = readFileSync(path, "utf8");
      const browserCount = calls.browser.length;
      const fetchCount = calls.fetch.length;
      const loadCount = loads(window);
      commit(window, status);
      await flush();
      assert.equal(calls.dialogs.length, 1);
      assert.equal(calls.dialogs[0].title, "Build could not load");
      assert.deepEqual(calls.dialogs[0].buttons, ["Try Again", "Quit"]);
      assert.equal(header(), credential);
      assert.equal(readFileSync(path, "utf8"), saved);
      dialog.resolve({ response: 0 });
      await flush();
      assert.equal(loads(window), loadCount + 1);
      assert.equal(window.url, APP_URL);
      assert.equal(header(), credential);
      assert.equal(readFileSync(path, "utf8"), saved);
      assert.equal(calls.browser.length, browserCount);
      assert.equal(calls.fetch.length, fetchCount);
    });
  }

  for (const committed of [false, true]) {
    for (const signal of ["did-fail-load", "loadURL rejection"]) {
      test(`post-${phase} ${signal} ${committed ? "after" : "before"} 200 commit preserves credentials and coalesces errors`, async (t) => {
        const dialog = deferred();
        const body = deferred();
        const retryLoad = phase === "refresh" ? 2 : 3;
        const { calls, window, header, path } = await retrying(t, phase, {
          showMessageBox: () => dialog.promise,
          loadURL: (target) => loads(target) === retryLoad ? body.promise : Promise.resolve(),
        });
        const credential = header();
        const saved = readFileSync(path, "utf8");
        if (committed) commit(window);
        if (signal === "did-fail-load") bodyFailure(window);
        else body.reject(loadError());
        await flush();
        assert.equal(calls.dialogs.length, 1);
        assert.equal(calls.dialogs[0].title, "Build could not load");
        assert.equal(header(), credential);
        assert.equal(readFileSync(path, "utf8"), saved);
        bodyFailure(window); // Duplicate failure callback for the same load.
        finish(window); // Electron can finish after reporting a failed body.
        if (signal === "did-fail-load") body.reject(loadError());
        await flush();
        assert.equal(calls.dialogs.length, 1);
        assert.equal(shown(window), 0, "failed finish cannot show or complete recovery");
        dialog.resolve({ response: 0 });
        await flush();
        assert.equal(loads(window), retryLoad + 1);
        assert.equal(header(), credential);
        assert.equal(calls.browser.length, phase === "oauth" ? 1 : 0);
      });
    }
  }

  test(`401 after a post-${phase} load-error retry resumes the existing authentication budget`, async (t) => {
    const loadDialog = deferred();
    const { calls, window } = await retrying(t, phase, {
      showMessageBox: (options) => options.title === "Build could not load" ? loadDialog.promise : new Promise(() => {}),
    });
    const fetchCount = calls.fetch.length;
    commit(window, 503);
    await flush();
    loadDialog.resolve({ response: 0 });
    await flush();
    unauthorized(calls, window, 103);
    await flush();
    assert.equal(calls.fetch.length, fetchCount, "a load retry never earns another refresh exchange");
    assert.equal(calls.browser.length, 1, "only the existing refresh-to-OAuth step may launch a browser");
    const authDialogs = calls.dialogs.filter((item) => item.title === "Build sign-in failed");
    assert.equal(authDialogs.length, phase === "oauth" ? 1 : 0);
  });
}

test("a 200 commit waits for successful finish before showing or resetting the auth budget", async (t) => {
  const { calls, window } = await retrying(t, "refresh");
  commit(window);
  assert.equal(shown(window), 0);
  unauthorized(calls, window, 102);
  await flush();
  assert.equal(calls.fetch.length, 2);
  assert.equal(calls.browser.length, 1, "unfinished 200 does not earn another refresh");
});

test("successful 200 plus same-document navigation and finish resets the authentication budget", async (t) => {
  const { calls, window } = await retrying(t, "oauth");
  commit(window);
  window.webContents.emit("did-start-navigation", {}, `${APP_URL}#/tasks/42`, true, true);
  finish(window);
  assert.equal(shown(window), 1, "hash navigation preserves the completed document identity");
  finish(window);
  assert.equal(shown(window), 1, "duplicate finish cannot repeat completion");
  window.webContents.emit("did-start-navigation", {}, APP_URL, false, true);
  unauthorized(calls, window, 103);
  await flush();
  assert.equal(calls.fetch.length, 4, "completed success earns a new refresh budget");
  assert.equal(calls.browser.length, 1);
});

for (const choice of [0, 1]) {
  test(`an older load dialog choice ${choice} cannot affect a newer completed manual reload`, async (t) => {
    const dialog = deferred();
    const { calls, window, header } = await retrying(t, "refresh", { showMessageBox: () => dialog.promise });
    commit(window, 503);
    await flush();
    const credential = header();
    await window.loadURL(APP_URL); // Native View > Reload has no main-process promise wrapper.
    finish(window); // A stale finish before this load commits is not success.
    assert.equal(shown(window), 0);
    commit(window);
    finish(window);
    assert.equal(shown(window), 1);
    const loadCount = loads(window);
    dialog.resolve({ response: choice });
    await flush();
    assert.equal(loads(window), loadCount);
    assert.equal(header(), credential);
    assert.equal(calls.quit, undefined);
    assert.deepEqual(calls.browser, []);
  });

  test(`load dialog choice ${choice} after Sign Out cannot retry or quit twice`, async (t) => {
    const dialog = deferred();
    const { calls, window, signOut, header } = await retrying(t, "refresh", { showMessageBox: () => dialog.promise });
    commit(window);
    bodyFailure(window);
    await flush();
    assert.equal(calls.dialogs.length, 1);
    const count = loads(window);
    await signOut().click();
    dialog.resolve({ response: choice });
    finish(window);
    await flush();
    assert.equal(header(), undefined);
    assert.equal(loads(window), count);
    assert.equal(calls.quit, 1);
  });
}

test("manual reload after a transient failure retains the post-OAuth budget until successful finish", async (t) => {
  const dialog = deferred();
  const { calls, window } = await retrying(t, "oauth", {
    showMessageBox: (options) => options.title === "Build could not load" ? dialog.promise : new Promise(() => {}),
  });
  commit(window, 503);
  await flush();
  await window.loadURL(APP_URL);
  commit(window);
  unauthorized(calls, window, 103);
  await flush();
  assert.equal(calls.fetch.length, 3);
  assert.equal(calls.browser.length, 1);
  assert.equal(calls.dialogs.filter((item) => item.title === "Build sign-in failed").length, 1);
  dialog.resolve({ response: 0 }); // Neither older load dialog nor its retry callback may open OAuth.
  await flush();
  assert.equal(calls.browser.length, 1);
});

for (const phase of ["refresh", "oauth"]) {
  test(`post-${phase} finish cannot precede the wrapped loadURL result`, async (t) => {
    const body = deferred();
    const retryLoad = phase === "refresh" ? 2 : 3;
    const { calls, window, header } = await retrying(t, phase, {
      loadURL: (target) => loads(target) === retryLoad ? body.promise : Promise.resolve(),
    });
    const credential = header();
    commit(window);
    finish(window);
    assert.equal(shown(window), 0, "finish alone cannot override an unsettled wrapped load");
    body.reject(loadError());
    await flush();
    assert.equal(calls.dialogs.length, 1);
    assert.equal(calls.dialogs[0].title, "Build could not load");
    assert.equal(header(), credential);
  });
}

test("wrapped load completes after both successful finish and promise fulfillment", async (t) => {
  const body = deferred();
  const { calls, window } = await retrying(t, "refresh", {
    loadURL: (target) => loads(target) === 2 ? body.promise : Promise.resolve(),
  });
  commit(window);
  finish(window);
  assert.equal(shown(window), 0);
  body.resolve();
  await flush();
  assert.equal(shown(window), 1);
  assert.deepEqual(calls.dialogs, []);
});
