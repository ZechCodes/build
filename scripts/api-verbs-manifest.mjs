#!/usr/bin/env node
// Writes fixtures/api/verbs-<previous minor>.json: the verbs and capabilities
// fixtures/api/ stated at the last commit before the current minor, read from
// git once so the contract tests can compare against it without git history.
//
// Run from anywhere in the repo whenever versions.json's `current` moves to a
// new minor (before or after committing the bump), and delete the manifest it
// replaces. spa/test/apiContract.test.js and bridge/tests/api_contract.rs
// refuse a manifest that is not the one for the release before `current`.
//
// A new major names the commit the previous release shipped from, because the
// break it carries lands in the fixtures before the number moves (2.0.0, the
// task rename, is the default branch's last 1.30.0):
//
//     node scripts/api-verbs-manifest.mjs origin/main
import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

if (process.argv.length > 3) {
  console.error("Usage: node scripts/api-verbs-manifest.mjs [<commit of the previous release>]");
  process.exit(1);
}

const repository = fileURLToPath(new URL("../", import.meta.url));
const git = (...args) => execFileSync("git", args, { cwd: repository, encoding: "utf8", maxBuffer: 64 << 20 });
const minorOf = (version) => version.split(".").slice(0, 2).join(".");
const releaseNumber = (version) => version.split(".").slice(0, 2).map(Number);
const earlier = (a, b) => {
  const [aMajor, aMinor] = releaseNumber(a);
  const [bMajor, bMinor] = releaseNumber(b);
  return aMajor < bMajor || (aMajor === bMajor && aMinor < bMinor);
};
const versionAt = (commit) => JSON.parse(git("show", `${commit}:fixtures/api/versions.json`)).current;

const current = JSON.parse(readFileSync(repository + "fixtures/api/versions.json", "utf8")).current;

// The newest first-parent commit still on an older minor. Walking only the
// commits that touched versions.json: the bump is the oldest of the run at the
// current minor, and the state before it is its first parent. A bump not yet
// committed leaves HEAD itself as the previous minor's last state.
let previous = process.argv[2] || "HEAD";
if (!process.argv[2]) {
  for (const commit of git("rev-list", "--first-parent", "HEAD", "--", "fixtures/api/versions.json").split("\n").filter(Boolean)) {
    if (earlier(versionAt(commit), current)) break;
    previous = `${commit}^1`;
  }
}
const previousVersion = versionAt(previous);
if (!earlier(previousVersion, current)) {
  console.error(`No commit before ${current} on this branch's first-parent history.`);
  process.exit(1);
}

const fixtureAt = (name) => JSON.parse(git("show", `${previous}:fixtures/api/v1/${name}`));
const verbs = git("ls-tree", "--name-only", `${previous}:fixtures/api/v1/`)
  .split("\n")
  .filter((name) => name.endsWith(".json"))
  .map(fixtureAt)
  .filter((body) => typeof body.method === "string")
  .map((body) => body.method)
  .sort();
const capabilities = [...fixtureAt("session.hello.json").result.capabilities].sort();

const manifest = {
  api_version: previousVersion,
  commit: git("rev-parse", "--short=8", previous).trim(),
  generated_by: "scripts/api-verbs-manifest.mjs",
  verbs,
  capabilities,
};
const path = `fixtures/api/verbs-${minorOf(previousVersion)}.json`;
writeFileSync(repository + path, JSON.stringify(manifest, null, 2) + "\n");

const stale = readdirSync(repository + "fixtures/api").filter((name) => /^verbs-.*\.json$/.test(name) && `fixtures/api/${name}` !== path);
console.log(`${path}: ${verbs.length} verbs, ${capabilities.length} capabilities at ${previousVersion} (${manifest.commit})`);
for (const name of stale) console.log(`delete fixtures/api/${name}: it is not the previous minor's`);
