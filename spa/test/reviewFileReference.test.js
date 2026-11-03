import { describe, expect, it } from "vitest";
import { resolveReviewFileReference } from "../src/core/reviewFileReference.js";
import { hashFromRoute, routeFromHash } from "../src/core/router.js";
const review = { snapshots: [{ number: 3, directories: [{ id: "workspace:root" }, { id: "workspace:root:deep" }] }] };
const reference = { revision: 3, mode: "commit", commit: "abcd", fileRef: "workspace:root:deep:src/a:b.js", side: "old", line: 42 };
describe("deferred review file references", () => {
  it("keeps a shareable unresolved reference until the revision arrives", () => {
    expect(resolveReviewFileReference(null, reference)).toEqual({ selection: reference, pending: true });
    const route = { name: "trackerTask", deviceId: "d", projectId: "p", taskId: "t", review: reference };
    expect(routeFromHash(hashFromRoute(route))).toEqual(route);
    expect(resolveReviewFileReference(review, reference)).toEqual({ selection: {
      revision: 3, mode: "commit", commit: "abcd", directoryId: "workspace:root:deep", path: "src/a:b.js", side: "old", line: 42,
    } });
  });
  it("accepts an encoded directory token and preserves the literal path", () => {
    expect(resolveReviewFileReference(review, { ...reference, fileRef: "workspace%3Aroot:src/a%3Ab.js" }).selection)
      .toMatchObject({ directoryId: "workspace:root", path: "src/a%3Ab.js" });
  });
  it("resolves an omitted directory only for a single-directory revision", () => {
    const one = { snapshots: [{ number: 3, directories: [{ id: "workspace:root" }] }] };
    expect(resolveReviewFileReference(one, { ...reference, fileRef: "src/a.js" }).selection)
      .toMatchObject({ directoryId: "workspace:root", path: "src/a.js" });
    expect(resolveReviewFileReference(review, { ...reference, fileRef: "src/a.js" }).missing).toBe(true);
  });
  it("falls back to revision All changes when no directory matches", () => {
    expect(resolveReviewFileReference(review, { ...reference, fileRef: "missing:src/a:b.js" })).toEqual({
      selection: { revision: 3, mode: "all" }, missing: true,
    });
  });
});
