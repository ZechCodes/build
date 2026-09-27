// The process the real-bridge test runs (test/lineProcess.js): no wait on it
// outlives the process, and stopping it leaves nothing it started behind.

import { afterEach, expect, it } from "vitest";
import { startLineProcess } from "./lineProcess.js";

const alive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

let started = null;
afterEach(async () => {
  await started?.stop();
  started = null;
});

it("fails a wait at once when the process ends before it answers", async () => {
  started = startLineProcess(process.execPath, ["-e", "process.stderr.write('the build failed'); process.exit(42)"]);
  const asked = Date.now();

  await expect(started.next((line) => line.ready, 600_000)).rejects.toThrow(/exit 42[\s\S]*the build failed/);
  expect(Date.now() - asked).toBeLessThan(10_000);
  await expect(started.next((line) => line.ready)).rejects.toThrow(/exit 42/);
});

it("fails a wait at once when the process ends while what it started holds its output", async () => {
  started = startLineProcess("sh", ["-c", "sleep 300 & exit 42"]);
  const asked = Date.now();

  await expect(started.next((line) => line.ready, 5_000)).rejects.toThrow(/exit 42/);
  expect(Date.now() - asked).toBeLessThan(2_000);
});

it("fails a wait at once when the command cannot start", async () => {
  started = startLineProcess("build-test-no-such-command", []);

  await expect(started.next((line) => line.ready, 600_000)).rejects.toThrow(/could not start[\s\S]*ENOENT/);
});

it("cancels a session's wait without consuming a later line", async () => {
  started = startLineProcess(process.execPath, ["-e", "setTimeout(() => console.log(JSON.stringify({ ready: true })), 100)"]);
  const closed = new AbortController();
  const pending = started.next((line) => line.ready, 20_000, closed.signal);

  closed.abort();
  await expect(pending).rejects.toThrow(/aborted/);
  await expect(started.next((line) => line.ready)).resolves.toMatchObject({ ready: true });
});

it("stops what the process started, not only the process", async () => {
  // A build script under cargo: a child of the child, still running.
  started = startLineProcess("sh", ["-c", "sleep 300 & echo \"{\\\"started\\\": $!}\"; wait"]);
  const { started: grandchild } = await started.next((line) => line.started);
  expect(alive(grandchild)).toBe(true);

  await started.stop();
  started = null;

  expect(alive(grandchild)).toBe(false);
});
