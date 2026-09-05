import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

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

test("only the application origin may navigate inside the window", () => {
  assert.equal(classifyNavigation("https://getbuild.ing/app/"), "internal");
  assert.equal(classifyNavigation("https://getbuild.ing/auth/passkey?next=%2Fapp%2F"), "internal");
  assert.equal(classifyNavigation("https://docs.getbuild.ing/guide"), "external");
  assert.equal(classifyNavigation("https://example.com/"), "external");
  assert.equal(classifyNavigation("http://getbuild.ing/app/"), "external");
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

test("the OAuth token is attached only to the application origin", () => {
  assert.deepEqual(
    authorizeRequestHeaders(
      "https://getbuild.ing/api/devices",
      { Accept: "application/json" },
      "access-token",
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
