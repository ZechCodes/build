// #119: a read asked for while the same read is out runs once more after it,
// never alongside it, and a burst of asks is one more read, not one each.
import { expect, it } from "vitest";
import { trailingRead } from "../src/core/trailingRead.js";

const deferred = () => {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
};

it("runs a burst asked during one read as a single read after it", async () => {
  const runs = [];
  const read = trailingRead(() => {
    const one = deferred();
    runs.push(one);
    return one.promise;
  });

  const first = read();
  read();
  read();
  read();
  expect(runs).toHaveLength(1);

  runs[0].resolve();
  await first;
  expect(runs).toHaveLength(2);
  runs[1].resolve();
  await Promise.resolve();
  expect(runs).toHaveLength(2);
});

it("keys the reads apart, so one project's read never holds up another's", async () => {
  const runs = [];
  const read = trailingRead((key) => {
    const one = deferred();
    runs.push([key, one]);
    return one.promise;
  });
  read("a");
  read("b");
  read("a");
  expect(runs.map(([key]) => key)).toEqual(["a", "b"]);
});

it("reads again after a read that threw", async () => {
  let calls = 0;
  const read = trailingRead(async () => {
    calls += 1;
    if (calls === 1) throw new Error("the wire went away");
  });
  await expect(read()).rejects.toThrow("the wire went away");
  await read();
  expect(calls).toBe(2);
});
