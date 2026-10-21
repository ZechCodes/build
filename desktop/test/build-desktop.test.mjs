import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";

const scriptUrl = new URL("../../scripts/build-desktop.mjs", import.meta.url);
const source = readFileSync(scriptUrl, "utf8")
  .replace(/^import .*;\n/gm, "")
  .replaceAll("import.meta.url", JSON.stringify(scriptUrl.href));

function runBuild(version) {
  const commands = [];
  const errors = [];
  const stopped = {};
  let exitCode = 0;
  try {
    runInNewContext(source, {
      URL,
      fileURLToPath,
      spawnSync: (command, args) => {
        commands.push([command, ...args]);
        return { status: 0 };
      },
      console: { log() {}, error: message => errors.push(message) },
      process: {
        argv: ["node", fileURLToPath(scriptUrl), "--dir"],
        execPath: "fixture-node",
        platform: "linux",
        versions: { node: version },
        exit: code => { exitCode = code; throw stopped; },
      },
    });
  } catch (error) {
    if (error !== stopped) throw error;
  }
  return { commands, errors, exitCode };
}

for (const version of [
  "20.20.0", "21.99.99", "22.12.0", "22.22.99", "22.23.0", "22.23.3",
  "23.0.0", "23.99.99", "24.0.0", "24.22.1", "25.99.99",
  "26.8.1", "26.12.1", "27.0.0", "28.0.0",
]) {
  test(`desktop build rejects Node ${version} before installing or packaging`, () => {
    const result = runBuild(version);
    assert.equal(result.exitCode, 1);
    assert.deepEqual(result.commands, []);
    assert.deepEqual(result.errors, [
      "Node.js >=22.23.4 <23 is required. See README.md (Build locally).",
    ]);
  });
}

for (const version of ["22.23.4", "22.23.5", "22.24.0", "22.99.99"]) {
  test(`desktop build accepts Node ${version}`, () => {
    const result = runBuild(version);
    assert.equal(result.exitCode, 0);
    assert.deepEqual(result.errors, []);
    assert.equal(result.commands.length, 2);
    assert.equal(result.commands[0][0], "npm");
    assert.equal(result.commands[1][0], "fixture-node");
  });
}
