// Type-check the new embedded telemetry code against its documented 0.86.1
// hook/context subset. The existing unrelated MCP client predates type checking.
// Pass an existing TypeScript compiler and @types root; never install packages.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const [compiler, typeRoots] = process.argv.slice(2);
assert.ok(compiler && typeRoots, "usage: node check-pi-compaction-types.mjs <typescript/bin/tsc> <@types directory>");
const source = readFileSync(new URL("../../src/harness/build-tools.ts", import.meta.url), "utf8");
const end = source.indexOf("function requiredEnvironment(");
assert.ok(end > 0, "telemetry prefix boundary is required");
let checked = source.slice(0, end);
for (const name of ["isObject", "isNonEmptyString"]) {
  const helper = source.match(new RegExp(`function ${name}\\([^]*?\\n\\}`));
  assert.ok(helper, `shared ${name} helper is required`);
  checked += `\n${helper[0]}\n`;
}
const directory = mkdtempSync(join(tmpdir(), "build-pi-typecheck-"));
try {
  const file = join(directory, "build-compaction.mjs");
  writeFileSync(file, checked);
  const result = spawnSync(process.execPath, [compiler, "--noEmit", "--allowJs", "--checkJs", "--skipLibCheck", "--strictNullChecks", "--target", "es2022", "--module", "nodenext", "--types", "node", "--typeRoots", typeRoots, file], { stdio: "inherit" });
  if (result.error) throw result.error;
  process.exitCode = result.status ?? 1;
} finally {
  rmSync(directory, { recursive: true, force: true });
}
