#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const arguments_ = process.argv.slice(2);
if (arguments_.includes("--help")) {
  console.log("Usage: node scripts/build-desktop.mjs [--dir]");
  process.exit(0);
}
if (arguments_.some((argument) => argument !== "--dir")) {
  console.error("Usage: node scripts/build-desktop.mjs [--dir]");
  process.exit(1);
}
const [major, minor] = process.versions.node.split(".").map(Number);
if (major < 22 || (major === 22 && minor < 12)) {
  console.error("Node.js 22.12 or newer is required. See README.md (Build locally).");
  process.exit(1);
}
if (!["linux", "darwin", "win32"].includes(process.platform)) {
  console.error("Desktop builds support Linux, macOS, and Windows.");
  process.exit(1);
}

const desktopDirectory = fileURLToPath(new URL("../desktop/", import.meta.url));
function run(command, args, shell = false) {
  const result = spawnSync(command, args, { cwd: desktopDirectory, stdio: "inherit", shell });
  if (result.error) console.error(result.error.message);
  if (result.status !== 0) process.exit(result.status ?? 1);
}

// npm.cmd needs a shell on Windows; every argument here is fixed, not user input.
run(process.platform === "win32" ? "npm.cmd" : "npm", ["ci", "--include=dev", "--no-audit", "--no-fund"], process.platform === "win32");
run(process.execPath, ["scripts/package-app.mjs", "--local-unsigned", ...arguments_]);
console.log(`\nDesktop output: ${desktopDirectory}dist`);
