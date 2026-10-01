// The production image builds the page in skriftapp/Containerfile's landing
// stage, which sees only what that stage copies in. An import under
// landing/src that reaches outside landing/ builds here, in a full checkout,
// and fails there; this reads the stage and the build context's ignores and
// says so first.
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const landingDir = fileURLToPath(new URL("..", import.meta.url));
const repoDir = path.dirname(landingDir.replace(/\/$/, ""));
const sourceDir = path.join(landingDir, "src");
const IMAGE_ROOT = "/build/repo";

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
    return /\.(astro|js|mjs|ts|css)$/.test(entry.name) ? [full] : [];
  });
}

// Every file the page's sources reach outside landing/, repo-relative.
function importsOutsideLanding() {
  const found = [];
  for (const file of sourceFiles(sourceDir)) {
    const text = readFileSync(file, "utf8");
    for (const pattern of SPECIFIERS) {
      for (const [, specifier] of text.matchAll(pattern)) {
        if (!specifier.startsWith(".")) continue;
        const target = path.resolve(path.dirname(file), specifier.replace(/[?#].*$/, ""));
        const fromRepo = path.relative(repoDir, target);
        if (!fromRepo.startsWith("landing/")) found.push({ file: path.relative(repoDir, file), target: fromRepo });
      }
    }
  }
  return found;
}

// The landing stage's COPY lines as [repo path, path in the image] pairs,
// relative destinations resolved against its WORKDIR.
function landingStageCopies() {
  const containerfile = readFileSync(path.join(repoDir, "skriftapp/Containerfile"), "utf8");
  const stage = containerfile.split(/^FROM\s/m).find((part) => /\bAS landing\s*$/m.test(part.split("\n")[0]));
  assert.ok(stage, "the Containerfile has a landing stage");
  let workdir = "/";
  const copies = [];
  for (const line of stage.split("\n")) {
    const [instruction, ...args] = line.trim().split(/\s+/);
    if (instruction === "WORKDIR") workdir = path.posix.resolve(workdir, args[0]);
    if (instruction !== "COPY" || args.some((arg) => arg.startsWith("--"))) continue;
    const destination = path.posix.resolve(workdir, args.at(-1));
    for (const source of args.slice(0, -1)) copies.push([source.replace(/\/$/, ""), destination]);
  }
  return copies;
}

// Copied to the same place under the image's repo root, as the relative
// import expects.
function copiedInPlace(target, copies) {
  return copies.some(([source, destination]) =>
    destination === path.posix.join(IMAGE_ROOT, source) && (target === source || target.startsWith(`${source}/`)));
}

function ignoredByContext(target) {
  const ignores = readFileSync(path.join(repoDir, ".containerignore"), "utf8").split("\n")
    .map((line) => line.trim()).filter((line) => line && !line.startsWith("#") && !line.startsWith("**"));
  return ignores.some((ignore) => target === ignore || target.startsWith(`${ignore.replace(/\/$/, "")}/`));
}

test("finds the page's sources and the landing stage", () => {
  assert.ok(sourceFiles(sourceDir).length > 10);
  assert.ok(landingStageCopies().some(([source]) => source === "landing"), "the stage copies landing/");
});

test("every import outside landing/ is copied in place by the image's landing stage", () => {
  const copies = landingStageCopies();
  const missing = importsOutsideLanding().filter(({ target }) => !copiedInPlace(target, copies) || ignoredByContext(target));
  assert.deepEqual(missing, [], "add a COPY to skriftapp/Containerfile's landing stage, or keep the file in landing/");
});
