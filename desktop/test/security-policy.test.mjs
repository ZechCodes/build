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

const APP_ALIASES = ["https://getbuild.ing/app", "https://getbuild.ing/app/", "https://getbuild.ing/app/?source=web#/tasks/42"];

const SIGN_IN_URL = "https://getbuild.ing/auth/login?next=/app/&view=signin";
const SIGN_IN_LOOKALIKE_URLS = [
  "https://getbuild.ing/auth/login",
  "https://getbuild.ing/auth/login?next=/app/",
  "https://getbuild.ing/auth/login?next=/&view=signin",
  "https://getbuild.ing/auth/login/?next=/app/&view=signin",
  "https://getbuild.ing/auth/login-other?next=/app/&view=signin",
  "https://getbuild.ing/%61uth/login?next=/app/&view=signin",
  "https://getbuild.ing/auth/%6cogin?next=/app/&view=signin",
  "https://getbuild.ing.evil.test/auth/login?next=/app/&view=signin",
  "https://getbuild.ing@evil.test/auth/login?next=/app/&view=signin",
  "https://getbuild.ing:8443/auth/login?next=/app/&view=signin",
  "http://getbuild.ing/auth/login?next=/app/&view=signin",
  `${SIGN_IN_URL}#other`,
  `${SIGN_IN_URL}&other=value`,
];

test("only the exact 401 sign-in CTA requests desktop authentication", () => {
  assert.equal(classifyNavigation(SIGN_IN_URL), "sign-in");
  for (const url of SIGN_IN_LOOKALIKE_URLS) assert.equal(classifyNavigation(url), "external", url);
});

const LOGOUT_LOOKALIKE_URLS = [
  "https://getbuild.ing/auth/logout/",
  "https://getbuild.ing/auth/logout-other",
  "https://getbuild.ing/%61uth/logout",
  "https://getbuild.ing/auth/%6cogout",
  "https://getbuild.ing.evil.test/auth/logout",
  "https://getbuild.ing@evil.test/auth/logout",
  "https://getbuild.ing:8443/auth/logout",
  "http://getbuild.ing/auth/logout",
  "https://example.com/auth/logout",
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

test("only the exact same-origin logout path requests desktop sign-out", () => {
  for (const suffix of ["", "?source=welcome", "#confirm", "?source=welcome#confirm"]) {
    assert.equal(classifyNavigation(`https://getbuild.ing/auth/logout${suffix}`), "sign-out");
  }
  for (const url of LOGOUT_LOOKALIKE_URLS) assert.equal(classifyNavigation(url), "external", url);
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

test("macOS retains native traffic lights inline with the 44px app header", () => {
  const options = createWindowOptions({ development: false, platform: "darwin" });

  assert.equal(options.titleBarStyle, "hidden");
  assert.deepEqual(options.titleBarOverlay, { height: 44 });
  assert.equal("trafficLightPosition" in options, false,
    "Electron centers using the actual native button height and keeps the OS horizontal inset");
  assert.equal("frame" in options, false, "keep the native window and controls");
});

test("other platforms retain their native title bar and controls", () => {
  for (const platform of ["linux", "win32"]) {
    const options = createWindowOptions({ development: false, platform });

    for (const name of ["titleBarStyle", "titleBarOverlay", "trafficLightPosition", "frame"]) {
      assert.equal(name in options, false, `${platform}: ${name}`);
    }
  }
});

test("macOS window chrome retains the same renderer security options", () => {
  const macOS = createWindowOptions({ development: false, platform: "darwin" });
  const native = createWindowOptions({ development: false, platform: "linux" });

  assert.deepEqual(macOS.webPreferences, native.webPreferences);
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
  for (const url of [...PUBLIC_URLS, ...APP_ALIASES, ...LOGOUT_LOOKALIKE_URLS, SIGN_IN_URL, ...SIGN_IN_LOOKALIKE_URLS, "https://getbuild.ing/auth/logout", "https://example.com/api/devices", "not a url"]) {
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
  const signOut = [];
  const signIn = [];
  securityPolicy.installNavigationPolicy(webContents, {
    openExternal: (...args) => external.push(args),
    loadInternal: (...args) => internal.push(args),
    signOut: (...args) => signOut.push(args),
    signIn: (...args) => signIn.push(args),
  });
  return { webContents, external, internal, signOut, signIn };
}

for (const eventName of ["will-navigate", "will-redirect", "window-open"]) {
  test(`${eventName} starts desktop sign-in instead of loading the web login`, () => {
    const { webContents, external, internal, signIn } = navigationHarness();
    if (eventName === "window-open") {
      assert.deepEqual(webContents.openWindow({ url: SIGN_IN_URL }), { action: "deny" });
    } else {
      let prevented = false;
      webContents.emit(eventName, { preventDefault: () => { prevented = true; } }, SIGN_IN_URL);
      assert.equal(prevented, true);
    }
    assert.deepEqual(signIn, [[]]);
    assert.deepEqual(external, []);
    assert.deepEqual(internal, []);
  });

  test(`${eventName} signs out locally instead of navigating to the logout page`, () => {
    const { webContents, external, internal, signOut } = navigationHarness();
    const url = "https://getbuild.ing/auth/logout";
    if (eventName === "window-open") {
      assert.deepEqual(webContents.openWindow({ url }), { action: "deny" });
    } else {
      let prevented = false;
      webContents.emit(eventName, { preventDefault: () => { prevented = true; } }, url);
      assert.equal(prevented, true);
    }
    assert.deepEqual(signOut, [[]]);
    assert.deepEqual(external, []);
    assert.deepEqual(internal, []);
  });

  test(`${eventName} sends non-app links to the system browser without credentials`, () => {
    for (const url of [...PUBLIC_URLS, ...LOGOUT_LOOKALIKE_URLS, ...SIGN_IN_LOOKALIKE_URLS, "https://getbuild.ing/api/devices", "https://example.com/", "mailto:support@getbuild.ing"]) {
      const { webContents, external, internal, signOut, signIn } = navigationHarness();
      if (eventName === "window-open") {
        assert.deepEqual(webContents.openWindow({ url }), { action: "deny" });
      } else {
        let prevented = false;
        webContents.emit(eventName, { preventDefault: () => { prevented = true; } }, url);
        assert.equal(prevented, true, url);
      }
      assert.deepEqual(external, [[url]], "browser receives just the URL, without headers");
      assert.deepEqual(internal, []);
      assert.deepEqual(signOut, []);
      assert.deepEqual(signIn, []);
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

for (const eventName of ["will-navigate", "will-redirect", "window-open"]) {
  test(`${eventName} normalizes exact web app aliases to the authenticated desktop URL`, () => {
    for (const url of APP_ALIASES) {
      assert.equal(classifyNavigation(url), "app-alias");
      const { webContents, external, internal, signIn } = navigationHarness();
      if (eventName === "window-open") webContents.openWindow({ url });
      else {
        let prevented = false;
        webContents.emit(eventName, { preventDefault: () => { prevented = true; } }, url, false, true);
        assert.equal(prevented, true);
      }
      assert.deepEqual(internal, [[APP_URL]]);
      assert.deepEqual(external, []);
      assert.deepEqual(signIn, []);
    }
  });
}

test("subframe redirects to app aliases are canceled without loading the main window", () => {
  const { webContents, external, internal } = navigationHarness();
  let prevented = false;
  webContents.emit("will-redirect", { preventDefault: () => { prevented = true; } }, "https://getbuild.ing/app/", false, false);
  assert.equal(prevented, true);
  assert.deepEqual(internal, []);
  assert.deepEqual(external, []);
});
