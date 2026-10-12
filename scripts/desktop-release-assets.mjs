import { lstatSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export function desktopReleaseAssets(version) {
  if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error("Invalid desktop release version");
  return [
    `Build-${version}-mac-arm64.dmg`, `Build-${version}-mac-arm64.zip`,
    "build-desktop-macos-arm64.zip",
    `Build-${version}-linux-x86_64.AppImage`, `Build-${version}-linux-amd64.deb`,
    "build-desktop-linux-x86_64.tar.gz",
    `Build-${version}-linux-arm64.AppImage`, `Build-${version}-linux-arm64.deb`,
    "build-desktop-linux-aarch64.tar.gz",
  ];
}

export function validateDesktopReleaseAssets(version, directory) {
  const expected = new Set(desktopReleaseAssets(version));
  const actual = new Set(readdirSync(directory));
  for (const name of actual) {
    if (!expected.has(name)) throw new Error(`Unexpected desktop release asset: ${name}`);
  }
  for (const name of expected) {
    if (!actual.has(name)) throw new Error(`Missing expected desktop release asset: ${name}`);
    const file = lstatSync(join(directory, name));
    if (!file.isFile() || file.size === 0) throw new Error(`Desktop asset must be a nonempty regular file: ${name}`);
  }
}

if (resolve(process.argv[1] || "") === fileURLToPath(import.meta.url)) {
  validateDesktopReleaseAssets(process.argv[2], process.argv[3]);
}
