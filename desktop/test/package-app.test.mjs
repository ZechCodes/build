import assert from "node:assert/strict";
import test from "node:test";

import { electronBuilderArguments, unsignedEnvironment } from "../scripts/package-app.mjs";

test("an unsigned macOS build receives an ad-hoc signature", () => {
  assert.deepEqual(
    electronBuilderArguments({
      platform: "darwin",
      signingConfigured: false,
      commandArguments: ["--mac", "--dir"],
    }),
    ["--mac", "--dir", "--config.mac.identity=-"],
  );
});

test("a configured production signature is not replaced", () => {
  assert.deepEqual(
    electronBuilderArguments({
      platform: "darwin",
      signingConfigured: true,
      commandArguments: ["--mac"],
    }),
    ["--mac"],
  );
});

test("non-macOS packages are unchanged", () => {
  assert.deepEqual(
    electronBuilderArguments({
      platform: "linux",
      signingConfigured: false,
      commandArguments: ["--linux"],
    }),
    ["--linux"],
  );
});

test("local Mac builds override configured signing and cannot publish or notarize", () => {
  assert.deepEqual(electronBuilderArguments({
    platform: "darwin",
    signingConfigured: true,
    commandArguments: ["--dir"],
    localUnsigned: true,
  }), ["--dir", "--publish", "never", "--config.mac.identity=-",
    "--config.mac.notarize=false", "--config.mac.hardenedRuntime=false"]);
});

test("local builds remove Apple and Windows signing credentials without changing the parent environment", () => {
  const environment = {
    PATH: "/usr/bin", CSC_LINK: "certificate", CSC_NAME: "identity",
    CSC_KEY_PASSWORD: "password", WIN_CSC_LINK: "windows-certificate",
    APPLE_ID: "account", APPLE_APP_SPECIFIC_PASSWORD: "password",
    CSC_IDENTITY_AUTO_DISCOVERY: "true",
  };
  assert.deepEqual(unsignedEnvironment(environment), {
    PATH: "/usr/bin", CSC_IDENTITY_AUTO_DISCOVERY: "false",
  });
  assert.equal(environment.CSC_LINK, "certificate");
});
