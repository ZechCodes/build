import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { parseAst } from "vite";
import { expect, it } from "vitest";

function dependencies(file) {
  return parseAst(readFileSync(file, "utf8")).body
    .filter((node) => node.source?.value.startsWith("."))
    .map((node) => resolve(dirname(file), node.source.value))
    .filter((path) => path.endsWith(".js"));
}

function dependencyPath(entry, target) {
  const paths = [[resolve(entry)]];
  const visited = new Set();
  for (const path of paths) {
    const file = path.at(-1);
    if (file === resolve(target)) return path;
    if (visited.has(file)) continue;
    visited.add(file);
    for (const dependency of dependencies(file)) paths.push([...path, dependency]);
  }
  return null;
}

it.each(["src/app.js", "src/connection.js"])(
  "keeps the device context dependency graph independent of %s",
  (target) => {
    expect(dependencyPath("src/core/deviceContexts.js", target)).toBeNull();
  },
);
