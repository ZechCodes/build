// A child process that speaks JSON lines, owned whole by the test that starts
// it (test/reconnectRepairWire.test.js runs the bridge this way).
//
// Owned whole: the child leads its own process group, so stopping it stops
// whatever it started as well — cargo's build scripts and compilers on a cold
// build — and a stop is not over until that group is empty. A child that
// cannot start, or ends, fails every wait on it at once, with what it said.

import { spawn } from "node:child_process";
import readline from "node:readline";

const STOP_GRACE_MS = 5000;
const POLL_MS = 50;
const EXIT_SETTLE_MS = 100;
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

/** Signal every process in the group; false once there is none left to. */
function signalGroup(leader, signal) {
  try {
    process.kill(-leader, signal);
    return true;
  } catch {
    return false;
  }
}

async function groupGone(leader) {
  for (let waited = 0; waited < STOP_GRACE_MS; waited += POLL_MS) {
    if (!signalGroup(leader, 0)) return true;
    await sleep(POLL_MS);
  }
  return !signalGroup(leader, 0);
}

export function startLineProcess(command, args, { cwd, onLine = () => false } = {}) {
  const child = spawn(command, args, { cwd, stdio: ["pipe", "pipe", "pipe"], detached: true });
  // What it said, kept for failure messages only.
  let said = "";
  const hear = (text) => { said = `${said}${text}`.slice(-4000); };
  child.stderr.on("data", hear);
  child.stdin.on("error", () => { /* it went first: `close` says how */ });

  const heard = [];
  const waiters = new Set();
  let ended = null;
  const end = (error) => {
    ended = ended || error;
    for (const waiter of waiters) waiter.fail(ended);
    waiters.clear();
  };
  const endedWith = (code, signal) => end(new Error(`${command} ended (${signal || `exit ${code}`}):\n${said}`));
  child.on("error", (error) => end(new Error(`${command} could not start: ${error.message}\n${said}`)));
  // Its exit, not its pipes closing: something it started can hold those open
  // long after it has failed. A moment first for what it said on the way out.
  child.on("exit", (code, signal) => setTimeout(() => endedWith(code, signal), EXIT_SETTLE_MS));
  child.on("close", endedWith);
  // A worker that exits under a test still takes the group with it.
  const orphaned = () => signalGroup(child.pid, "SIGKILL");
  if (child.pid) process.once("exit", orphaned);

  readline.createInterface({ input: child.stdout }).on("line", (text) => {
    let line;
    try {
      line = JSON.parse(text);
    } catch {
      return void hear(`${text}\n`);
    }
    if (onLine(line)) return;
    const waiter = [...waiters].find((candidate) => candidate.match(line));
    if (!waiter) return void heard.push(line);
    waiters.delete(waiter);
    waiter.resolve(line);
  });

  /** The first line `match` accepts, however long ago it came. */
  const next = (match, waitMs = 20000) => {
    const early = heard.findIndex(match);
    if (early >= 0) return Promise.resolve(heard.splice(early, 1)[0]);
    if (ended) return Promise.reject(ended);
    return new Promise((resolve, reject) => {
      const waiter = {
        match,
        resolve: (line) => { clearTimeout(timer); resolve(line); },
        fail: (error) => { clearTimeout(timer); reject(error); },
      };
      const timer = setTimeout(() => {
        waiters.delete(waiter);
        reject(new Error(`${command} did not answer:\n${said}`));
      }, waitMs);
      waiters.add(waiter);
    });
  };

  const send = (line) => child.stdin.write(`${JSON.stringify(line)}\n`);

  /** Stop the child and everything it started, and wait until they have gone. */
  const stop = async () => {
    process.removeListener("exit", orphaned);
    child.stdin.end();
    if (!child.pid) return;
    signalGroup(child.pid, "SIGTERM");
    if (await groupGone(child.pid)) return;
    signalGroup(child.pid, "SIGKILL");
    if (!(await groupGone(child.pid))) throw new Error(`${command} left processes behind in group ${child.pid}`);
  };

  return { child, next, send, stop };
}
