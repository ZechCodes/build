import { chmod, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { chromiumExecutable } from "./browser/chromiumExecutable.mjs";

let directory;
beforeEach(async () => {
  directory = await mkdtemp(join(tmpdir(), "build-chromium-lookup-"));
  vi.stubEnv("PATH", directory);
  vi.stubEnv("CHROMIUM_PATH", "");
});
afterEach(async () => {
  vi.unstubAllEnvs();
  await rm(directory, { recursive: true, force: true });
});

async function executable(name) {
  const path = join(directory, name);
  await writeFile(path, "#!/bin/sh\nexit 0\n");
  await chmod(path, 0o755);
  return path;
}

it.each(["chromium", "chromium-browser", "google-chrome", "google-chrome-stable"])(
  "finds executable %s on PATH without a fixed install location", async (name) => {
    const path = await executable(name);
    expect(await chromiumExecutable()).toBe(path);
  },
);

it("prefers an explicitly configured executable", async () => {
  await executable("chromium");
  const configured = await executable("custom-browser");
  vi.stubEnv("CHROMIUM_PATH", configured);
  expect(await chromiumExecutable()).toBe(configured);
});

it("ignores a non-executable candidate and explains when no browser is available", async () => {
  await writeFile(join(directory, "chromium"), "not executable");
  await expect(chromiumExecutable()).rejects.toThrow("Chromium or Chrome on PATH (or CHROMIUM_PATH)");
});
