// The app image builds the bundle in skriftapp/Containerfile's spa stage,
// which sees only what that stage copies in. An import under spa/src that
// reaches outside spa/ builds here, in a full checkout, and fails there (#913's
// fixtures/api/http manifest did); this reads the stage and the build
// context's ignores and says so first.
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const spaDir = fileURLToPath(new URL("..", import.meta.url)).replace(/\/$/, "");
const repoDir = path.dirname(spaDir);
const sourceDir = path.join(spaDir, "src");

// Relative specifiers in module imports, CSS @import and url(), and
// new URL(…, import.meta.url).
const SPECIFIERS = [
  /\bimport\s[^"']*?\bfrom\s*["']([^"']+)["']/g,
  /\bimport\s*\(?\s*["']([^"']+)["']/g,
  /@import\s+(?:url\()?\s*["']?([^"')\s;]+)/g,
  /\burl\(\s*["']?([^"')\s]+)/g,
  /\bnew URL\(\s*["']([^"']+)["']\s*,\s*import\.meta\.url/g,
];

function sourceFiles(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) return sourceFiles(full);
    return /\.(js|mjs|ts|css|html)$/.test(entry.name) ? [full] : [];
  });
}

// Every file the bundle's sources reach outside spa/, repo-relative.
function importsOutsideSpa() {
  const found = [];
  for (const file of [...sourceFiles(sourceDir), path.join(spaDir, "index.html")]) {
    const text = readFileSync(file, "utf8");
    for (const pattern of SPECIFIERS) {
      for (const [, specifier] of text.matchAll(pattern)) {
        if (!specifier.startsWith(".")) continue;
        const target = path.resolve(path.dirname(file), specifier.replace(/[?#].*$/, ""));
        const fromRepo = path.relative(repoDir, target);
        if (!fromRepo.startsWith("spa/")) found.push({ file: path.relative(repoDir, file), target: fromRepo });
      }
    }
  }
  return found;
}

// The spa stage's COPY lines as [repo path, path in the image] pairs,
// relative destinations resolved against its WORKDIR.
function spaStage() {
  const containerfile = readFileSync(path.join(repoDir, "skriftapp/Containerfile"), "utf8");
  const stage = containerfile.split(/^FROM\s/m).find((part) => /\bAS spa\s*$/m.test(part.split("\n")[0]));
  expect(stage, "the Containerfile has a spa stage").toBeTruthy();
  let workdir = "/";
  const copies = [];
  for (const line of stage.split("\n")) {
    const [instruction, ...args] = line.trim().split(/\s+/);
    if (instruction === "WORKDIR") workdir = path.posix.resolve(workdir, args[0]);
    if (instruction !== "COPY" || args.some((arg) => arg.startsWith("--"))) continue;
    const destination = path.posix.resolve(workdir, args.at(-1));
    for (const source of args.slice(0, -1)) copies.push([source.replace(/\/$/, ""), destination]);
  }
  return { root: path.posix.resolve(workdir), copies };
}

// Copied under the stage's repo root (its first WORKDIR) at the path the
// relative import expects.
function copiedInPlace(target, { root, copies }) {
  return copies.some(([source, destination]) =>
    destination === path.posix.join(root, source) && (target === source || target.startsWith(`${source}/`)));
}

function ignoredByContext(target) {
  const ignores = readFileSync(path.join(repoDir, ".containerignore"), "utf8").split("\n")
    .map((line) => line.trim()).filter((line) => line && !line.startsWith("#") && !line.startsWith("**"));
  return ignores.some((ignore) => target === ignore || target.startsWith(`${ignore.replace(/\/$/, "")}/`));
}

describe("the app image's spa stage", () => {
  it("finds the bundle's sources, the stage, and the import #913 added", () => {
    expect(sourceFiles(sourceDir).length).toBeGreaterThan(50);
    expect(spaStage().copies.some(([source]) => source === "spa")).toBe(true);
    expect(importsOutsideSpa().map(({ target }) => target)).toContain("fixtures/api/http/resource-manifest.json");
  });

  it("copies every import outside spa/ in place, from a context that keeps it", () => {
    const stage = spaStage();
    const missing = importsOutsideSpa().filter(({ target }) => !copiedInPlace(target, stage) || ignoredByContext(target));
    expect(missing, "add a COPY to skriftapp/Containerfile's spa stage, or keep the file in spa/").toEqual([]);
  });
});
