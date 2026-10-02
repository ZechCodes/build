import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { readFile } from "node:fs/promises";
import test from "node:test";

import * as securityPolicy from "../src/security-policy.mjs";
import {
  APP_URL,
  authorizeRequestHeaders,
  classifyNavigation,
  createPermissionPolicy,
  createWindowOptions,
} from "../src/security-policy.mjs";

test("the desktop app opens the production web client", () => {
  assert.equal(APP_URL, "https://getbuild.ing/app/desktop");
});

const PUBLIC_URLS = [
  "https://getbuild.ing/",
  "https://getbuild.ing/docs/guide",
  "https://getbuild.ing/app/",
  "https://getbuild.ing/auth/passkey?next=%2Fapp%2F",
  "https://getbuild.ing/oauth/token",
  "https://getbuild.ing/app/downloads",
  "https://getbuild.ing/app/static/version.json",
  "https://getbuild.ing/app/desktop/",
  "https://getbuild.ing/app/desktop-other",
  "https://getbuild.ing/app/desktop/../downloads",
  "https://getbuild.ing/%61pp/desktop",
  "https://getbuild.ing/api/../docs",
  "https://getbuild.ing/api-other/devices",
];

test("only the exact desktop app path may navigate inside the window", () => {
  for (const suffix of ["", "?source=desktop", "#/tasks/42", "?source=desktop#/tasks/42"]) {
    assert.equal(classifyNavigation(`${APP_URL}${suffix}`), "internal");
  }
  for (const url of PUBLIC_URLS) assert.equal(classifyNavigation(url), "external", url);
  assert.equal(classifyNavigation("https://getbuild.ing/api/devices"), "external");
  assert.equal(classifyNavigation("https://docs.getbuild.ing/guide"), "external");
  assert.equal(classifyNavigation("https://example.com/"), "external");
  assert.equal(classifyNavigation("https://getbuild.ing.evil.test/app/desktop"), "external");
  assert.equal(classifyNavigation("https://getbuild.ing@evil.test/app/desktop"), "external");
  assert.equal(classifyNavigation("https://getbuild.ing:8443/app/desktop"), "external");
  assert.equal(classifyNavigation("http://getbuild.ing/app/desktop"), "external");
  assert.equal(classifyNavigation("mailto:support@getbuild.ing"), "external");
  assert.equal(classifyNavigation("file:///etc/passwd"), "deny");
  assert.equal(classifyNavigation("javascript:alert(1)"), "deny");
  assert.equal(classifyNavigation("not a url"), "deny");
});

test("the renderer has no Node, preload, or unsandboxed access", () => {
  const options = createWindowOptions({ development: false });

  assert.equal(options.webPreferences.nodeIntegration, false);
  assert.equal(options.webPreferences.nodeIntegrationInWorker, false);
  assert.equal(options.webPreferences.contextIsolation, true);
  assert.equal(options.webPreferences.allowRunningInsecureContent, false);
  assert.equal(options.webPreferences.navigateOnDragDrop, false);
  assert.equal(options.webPreferences.sandbox, true);
  assert.equal(options.webPreferences.webSecurity, true);
  assert.equal(options.webPreferences.webviewTag, false);
  assert.equal(options.webPreferences.devTools, false);
  assert.equal("preload" in options.webPreferences, false);
});

test("development builds may expose Chromium developer tools", () => {
  const options = createWindowOptions({ development: true });

  assert.equal(options.webPreferences.devTools, true);
});

test("only notifications from the application origin receive permission", () => {
  const permissionAllowed = createPermissionPolicy();

  assert.equal(permissionAllowed("notifications", "https://getbuild.ing/app/"), true);
  assert.equal(permissionAllowed("notifications", "https://example.com/"), false);
  assert.equal(permissionAllowed("media", "https://getbuild.ing/app/"), false);
  assert.equal(permissionAllowed("clipboard-read", "https://getbuild.ing/app/"), false);
  assert.equal(permissionAllowed("notifications", "not a url"), false);
});

test("the OAuth token authenticates the desktop document and API fetches", () => {
  for (const suffix of ["", "?source=desktop#/tasks/42"]) {
    assert.deepEqual(
      authorizeRequestHeaders(`${APP_URL}${suffix}`, { Accept: "text/html" }, "access-token", "mainFrame"),
      { Accept: "text/html", Authorization: "Bearer access-token" },
    );
  }
  assert.deepEqual(
    authorizeRequestHeaders(
      "https://getbuild.ing/api/devices",
      { Accept: "application/json" },
      "access-token",
      "xhr",
    ),
    { Accept: "application/json", Authorization: "Bearer access-token" },
  );
  assert.deepEqual(
    authorizeRequestHeaders(
      "https://example.com/",
      { Accept: "application/json" },
      "access-token",
    ),
    { Accept: "application/json" },
  );
});

test("non-app requests receive no Bearer, including redirected headers", () => {
  for (const url of [...PUBLIC_URLS, "https://example.com/api/devices", "not a url"]) {
    for (const authorizationName of ["Authorization", "authorization", "AUTHORIZATION"]) {
      const headers = { Accept: "text/html", [authorizationName]: "Bearer access-token" };
      for (const resourceType of ["mainFrame", "xhr"]) {
        assert.deepEqual(authorizeRequestHeaders(url, headers, "access-token", resourceType),
          { Accept: "text/html" }, `${url} (${resourceType})`);
      }
      assert.equal(headers[authorizationName], "Bearer access-token");
    }
    assert.deepEqual(authorizeRequestHeaders(url, { Accept: "text/html" }, "access-token", "mainFrame"),
      { Accept: "text/html" }, url);
  }
});

test("API navigation and missing tokens cannot attach a Bearer", () => {
  const headers = { Accept: "application/json", Authorization: "Bearer old-token" };
  for (const resourceType of ["mainFrame", "subFrame", "script", undefined]) {
    assert.deepEqual(authorizeRequestHeaders("https://getbuild.ing/api/devices", headers, "access-token", resourceType),
      { Accept: "application/json" });
  }
  assert.deepEqual(authorizeRequestHeaders(APP_URL, headers, null, "mainFrame"),
    { Accept: "application/json" });
});

test("the desktop path receives a Bearer only as the main-frame document", () => {
  const headers = { Accept: "*/*", authorization: "Bearer old-token" };
  for (const resourceType of ["subFrame", "image", "script", "stylesheet", "xhr", "other", undefined]) {
    assert.deepEqual(authorizeRequestHeaders(APP_URL, headers, "access-token", resourceType),
      { Accept: "*/*" }, resourceType);
  }
});

function authorizationHarness(getAccessToken = () => "access-token") {
  let filter;
  let listener;
  securityPolicy.installAuthorizationHeader({
    onBeforeSendHeaders: (requestFilter, callback) => {
      filter = requestFilter;
      listener = callback;
    },
  }, getAccessToken);
  return {
    filter,
    requestHeaders(details) {
      let result;
      listener(details, (response) => { result = response.requestHeaders; });
      return result;
    },
  };
}

test("the header hook covers all URLs so cross-origin redirects reach the policy", () => {
  const { filter } = authorizationHarness();
  assert.deepEqual(filter, { urls: ["<all_urls>"] });
});

test("the header hook removes any Authorization case from other hosts", () => {
  const { requestHeaders } = authorizationHarness();
  for (const url of ["https://example.com/api/devices", "https://docs.getbuild.ing/app/desktop", "http://example.com/"]) {
    for (const name of ["Authorization", "authorization", "AUTHORIZATION", "aUtHoRiZaTiOn"]) {
      assert.deepEqual(requestHeaders({
        url,
        resourceType: "xhr",
        requestHeaders: { Accept: "application/json", [name]: "Bearer access-token" },
      }), { Accept: "application/json" }, `${url}: ${name}`);
    }
  }
});

test("the header hook uses the current token only for the two allowed request types", () => {
  let token = "first-token";
  const { requestHeaders } = authorizationHarness(() => token);
  for (const [url, resourceType] of [[APP_URL, "mainFrame"], ["https://getbuild.ing/api/devices", "xhr"]]) {
    const details = { url, resourceType, requestHeaders: { Accept: "*/*" } };
    assert.deepEqual(requestHeaders(details), { Accept: "*/*", Authorization: `Bearer ${token}` });
    token = "refreshed-token";
    assert.deepEqual(requestHeaders(details), { Accept: "*/*", Authorization: "Bearer refreshed-token" });
  }
  assert.deepEqual(requestHeaders({ url: APP_URL, resourceType: "image", requestHeaders: {} }), {});
});

function navigationHarness() {
  const webContents = new EventEmitter();
  webContents.setWindowOpenHandler = (handler) => { webContents.openWindow = handler; };
  const external = [];
  const internal = [];
  securityPolicy.installNavigationPolicy(webContents, {
    openExternal: (...args) => external.push(args),
    loadInternal: (...args) => internal.push(args),
  });
  return { webContents, external, internal };
}

for (const eventName of ["will-navigate", "will-redirect", "window-open"]) {
  test(`${eventName} sends non-app links to the system browser without credentials`, () => {
    for (const url of [...PUBLIC_URLS, "https://getbuild.ing/api/devices", "https://example.com/", "mailto:support@getbuild.ing"]) {
      const { webContents, external, internal } = navigationHarness();
      if (eventName === "window-open") {
        assert.deepEqual(webContents.openWindow({ url }), { action: "deny" });
      } else {
        let prevented = false;
        webContents.emit(eventName, { preventDefault: () => { prevented = true; } }, url);
        assert.equal(prevented, true, url);
      }
      assert.deepEqual(external, [[url]], "browser receives just the URL, without headers");
      assert.deepEqual(internal, []);
    }
  });

  test(`${eventName} retains desktop routes and denies unsafe schemes`, () => {
    for (const url of [APP_URL, `${APP_URL}?source=desktop#/tasks/42`, "file:///etc/passwd", "javascript:alert(1)", "not a url"]) {
      const { webContents, external, internal } = navigationHarness();
      const isApp = url.startsWith(APP_URL);
      if (eventName === "window-open") {
        assert.deepEqual(webContents.openWindow({ url }), { action: "deny" });
        assert.deepEqual(internal, isApp ? [[url]] : []);
      } else {
        let prevented = false;
        webContents.emit(eventName, { preventDefault: () => { prevented = true; } }, url);
        assert.equal(prevented, !isApp);
        assert.deepEqual(internal, []);
      }
      assert.deepEqual(external, []);
    }
  });
}

test("embedded webviews remain disabled", () => {
  const { webContents } = navigationHarness();
  let prevented = false;
  webContents.emit("will-attach-webview", { preventDefault: () => { prevented = true; } });
  assert.equal(prevented, true);
});

test("packaged binaries disable Electron escape hatches", async () => {
  const packageJson = JSON.parse(
    await readFile(new URL("../package.json", import.meta.url), "utf8"),
  );

  assert.deepEqual(packageJson.build.electronFuses, {
    runAsNode: false,
    enableCookieEncryption: true,
    enableNodeOptionsEnvironmentVariable: false,
    enableNodeCliInspectArguments: false,
    enableEmbeddedAsarIntegrityValidation: true,
    onlyLoadAppFromAsar: true,
    grantFileProtocolExtraPrivileges: false,
  });
});
