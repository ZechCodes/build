import assert from "node:assert/strict";
import test from "node:test";

import { electronBuilderArguments } from "../scripts/package-app.mjs";

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
