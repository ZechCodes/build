import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { desktopReleaseAssets, validateDesktopReleaseAssets } from "../../scripts/desktop-release-assets.mjs";

const expected = [
  "Build-0.1.1-mac-arm64.dmg", "Build-0.1.1-mac-arm64.zip",
  "build-desktop-macos-arm64.zip",
  "Build-0.1.1-linux-x86_64.AppImage", "Build-0.1.1-linux-amd64.deb",
  "build-desktop-linux-x86_64.tar.gz",
  "Build-0.1.1-linux-arm64.AppImage", "Build-0.1.1-linux-arm64.deb",
  "build-desktop-linux-aarch64.tar.gz",
];

test("release contract includes Apple Silicon and both Linux architectures", () => {
  assert.deepEqual(desktopReleaseAssets("0.1.1").sort(), expected.toSorted());
});

function fixture(t) {
  const directory = mkdtempSync(join(tmpdir(), "desktop-release-assets-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  for (const name of expected) writeFileSync(join(directory, name), "fixture");
  return directory;
}

test("complete supported assets pass publication validation", (t) => {
  validateDesktopReleaseAssets("0.1.1", fixture(t));
});

test("missing Linux x64 blocks publication", (t) => {
  const directory = fixture(t);
  rmSync(join(directory, "build-desktop-linux-x86_64.tar.gz"));
  assert.throws(() => validateDesktopReleaseAssets("0.1.1", directory), /missing|expected/i);
});

test("empty supported artifact blocks publication", (t) => {
  const directory = fixture(t);
  writeFileSync(join(directory, "Build-0.1.1-mac-arm64.dmg"), "");
  assert.throws(() => validateDesktopReleaseAssets("0.1.1", directory), /empty|nonempty/i);
});

test("Intel Mac artifact blocks publication", (t) => {
  const directory = fixture(t);
  writeFileSync(join(directory, "build-desktop-macos-x86_64.zip"), "stale");
  assert.throws(() => validateDesktopReleaseAssets("0.1.1", directory), /unexpected/i);
});

test("wrong version artifacts block publication", (t) => {
  assert.throws(() => validateDesktopReleaseAssets("0.1.2", fixture(t)), /missing|expected/i);
});

test("a symlink cannot stand in for a verified distributable", (t) => {
  const directory = fixture(t);
  const name = "build-desktop-linux-aarch64.tar.gz";
  rmSync(join(directory, name));
  symlinkSync("Build-0.1.1-mac-arm64.zip", join(directory, name));
  assert.throws(() => validateDesktopReleaseAssets("0.1.1", directory), /regular file/i);
});

function assertReleaseWorkflowContract(workflow) {
  workflow = workflow.replace(/\r\n/g, "\n");
  const macJob = workflow.split("  build:\n")[1].split("  build-linux:\n")[0];
  const linuxJob = workflow.split("  build-linux:\n")[1].split("  publish:\n")[0];
  const publishJob = workflow.split("  publish:\n")[1];
  assert.match(macJob, /arch: arm64/);
  assert.doesNotMatch(macJob, /arch: x64|macos-x86_64|macos-15-intel/);
  assert.match(linuxJob, /arch: x64/);
  assert.match(linuxJob, /arch: arm64/);
  for (const gate of ["forceCodeSigning=true", "mac.notarize=true", "codesign --verify --deep --strict", "xcrun stapler validate", "spctl --assess"]) {
    assert.ok(macJob.includes(gate), gate);
  }
  assert.match(publishJob, /needs: \[version, build, build-linux\]/);
  assert.match(publishJob, /actions\/checkout@/);
  assert.ok(publishJob.indexOf("desktop-release-assets.mjs") < publishJob.indexOf("sha256sum"));
  assert.match(publishJob, /cosign verify-blob/);
}

for (const [name, newline] of [["LF", "\n"], ["CRLF", "\r\n"]]) {
  test(`release workflow keeps supported platform and signing gates with ${name} newlines`, () => {
    const workflow = readFileSync(new URL("../../.github/workflows/release-desktop.yml", import.meta.url), "utf8");
    assertReleaseWorkflowContract(workflow.replace(/\r?\n/g, newline));
  });
}

test("default Mac packages are ARM64 and package versions agree", () => {
  const pkg = JSON.parse(readFileSync(new URL("../package.json", import.meta.url)));
  const lock = JSON.parse(readFileSync(new URL("../package-lock.json", import.meta.url)));
  assert.deepEqual(pkg.build.mac.target, [
    { target: "dmg", arch: ["arm64"] }, { target: "zip", arch: ["arm64"] },
  ]);
  assert.equal(pkg.version, "0.1.4");
  assert.equal(pkg.version, lock.version);
  assert.equal(pkg.version, lock.packages[""].version);
});
