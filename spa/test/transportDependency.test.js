import { realpathSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";
import { generateSessionKey } from "@build/secure-transport";

it("resolves the in-tree binding and initializes its locked sodium", async () => {
  const require = createRequire(import.meta.url);
  const expected = fileURLToPath(new URL("../../transport/js/src/index.js", import.meta.url));
  expect(realpathSync(require.resolve("@build/secure-transport"))).toBe(expected);
  expect(Buffer.from(await generateSessionKey(), "base64")).toHaveLength(32);
});
