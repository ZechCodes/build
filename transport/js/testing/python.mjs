import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// Overrides are deliberate compatibility probes; normal tests use this checkout.
export function python(request) {
  const reference = path.resolve(process.env.BUILD_SECURE_TRANSPORT_PY ||
    fileURLToPath(new URL("../../python/", import.meta.url)));
  const required = ["pyproject.toml", "uv.lock", "src/build_secure_transport/__init__.py",
    "tests/interop_helper.py"];
  for (const file of required) {
    if (!existsSync(path.join(reference, file))) {
      throw new Error(`required Python reference missing: ${path.join(reference, file)}`);
    }
  }
  const uv = process.env.BST_UV || "uv";
  const result = spawnSync(uv, ["run", "--frozen", "--quiet", "--project", reference,
    "python", path.join(reference, "tests/interop_helper.py")], {
    cwd: reference,
    env: { ...process.env, PYTHONPATH: path.join(reference, "src") },
    input: JSON.stringify(request),
    encoding: "utf8"
  });
  if (result.error) {
    throw new Error(`required uv executable '${uv}' could not run: ${result.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(`Python interop helper failed (exit ${result.status}, signal ${result.signal}): ${result.stderr}`);
  }
  return JSON.parse(result.stdout);
}
