// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { pairCodeFromHash, watchPairLinks } from "../src/core/pairLink.js";

// The approve link `build-bridge pair` prints is /app/#/pair/<code> (#319): the
// code rides in the fragment, which no request carries, and the app takes it
// out of the address before anything else reads the hash.

beforeEach(() => history.replaceState(null, "", "/app/"));
afterEach(() => history.replaceState(null, "", "/app/"));

it("reads a pairing code from the approve link's fragment", () => {
  expect(pairCodeFromHash("#/pair/ZSAC-ABU6")).toBe("ZSAC-ABU6");
  expect(pairCodeFromHash("#/pair/zsac-abu6")).toBe("ZSAC-ABU6");
  expect(pairCodeFromHash("#/pair/COMPOSE-PAIR-2")).toBe("COMPOSE-PAIR-2");
});

it("reads nothing from any other fragment, or from a code that could carry markup", () => {
  for (const hash of ["", "#/inbox", "#/pair", "#/pair/", "#/pair/A/B", "#/pair/<b>x</b>", "#/pair/A%20B", `#/pair/${"A".repeat(40)}`]) {
    expect(pairCodeFromHash(hash)).toBeNull();
  }
});

it("takes the code the page opened on out of the address and hands it on", () => {
  history.replaceState(null, "", "/app/?x=1#/pair/ZSAC-ABU6");
  const open = vi.fn();
  const stop = watchPairLinks(window, open);
  expect(open).toHaveBeenCalledWith("ZSAC-ABU6");
  expect(location.pathname + location.search + location.hash).toBe("/app/?x=1#/account/devices");
  stop();
});

it("hands on a link followed while the app is already open, before the router reads it", () => {
  const open = vi.fn();
  const stop = watchPairLinks(window, open);
  expect(open).not.toHaveBeenCalled();
  const seenByRouter = [];
  const router = () => seenByRouter.push(location.hash);
  window.addEventListener("hashchange", router);
  location.hash = "#/pair/WXYZ-4F2K";
  window.dispatchEvent(new HashChangeEvent("hashchange"));
  expect(open).toHaveBeenCalledWith("WXYZ-4F2K");
  expect(seenByRouter.at(-1)).toBe("#/account/devices");
  window.removeEventListener("hashchange", router);
  stop();
});

it("leaves every other address alone", () => {
  history.replaceState(null, "", "/app/#/inbox");
  const open = vi.fn();
  const stop = watchPairLinks(window, open);
  window.dispatchEvent(new HashChangeEvent("hashchange"));
  expect(open).not.toHaveBeenCalled();
  expect(location.hash).toBe("#/inbox");
  stop();
});
