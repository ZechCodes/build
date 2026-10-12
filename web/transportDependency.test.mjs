import assert from "node:assert/strict";
import { realpathSync } from "node:fs";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { ready } from "@build/secure-transport";

test("Node resolves the binding within this checkout and loads its locked sodium", async () => {
  const expected = fileURLToPath(new URL("../transport/js/src/index.js", import.meta.url));
  assert.equal(realpathSync(fileURLToPath(import.meta.resolve("@build/secure-transport"))), expected);
  await ready();
});
