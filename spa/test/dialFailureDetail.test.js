// What a failed dial says about itself in the diagnostics: which step it died
// on, what kind of thing was thrown, the words only when they are words this
// app or a browser's network stack wrote, and whether the browser believed it
// was online. A server's error text never makes it into a shared report.

import { describe, it, expect } from "vitest";
import { dialFailureDetail } from "../src/core/dialFailureDetail.js";

describe("dialFailureDetail", () => {
  it("names the step a rendezvous failure was tagged with", () => {
    const error = Object.assign(new Error("the relay did not answer"), { dialStep: "relay-open" });
    expect(dialFailureDetail(error, { online: true })).toEqual({
      step: "relay-open",
      error: "Error",
      message: "the relay did not answer",
      online: true,
    });
  });

  it("keeps a browser's own network-error wording", () => {
    const error = Object.assign(new TypeError("Failed to fetch"), { dialStep: "gateway-token" });
    expect(dialFailureDetail(error, { online: false })).toEqual({
      step: "gateway-token",
      error: "TypeError",
      message: "Failed to fetch",
      online: false,
    });
  });

  it("keeps the app's own words for a send on a socket that had gone", () => {
    const error = Object.assign(new Error("the relay socket closed"), { dialStep: "relay-send" });
    expect(dialFailureDetail(error, { online: true })).toMatchObject({ step: "relay-send", message: "the relay socket closed" });
  });

  it("drops words it did not write, keeping only what kind of error it was", () => {
    const error = new Error("private response with credentials");
    expect(dialFailureDetail(error, { online: true })).toEqual({
      step: null,
      error: "Error",
      message: null,
      online: true,
    });
  });

  it("names a socket's error event by its type, since it is not an Error", () => {
    const event = Object.assign(new Event("error"), { dialStep: "relay-socket" });
    expect(dialFailureDetail(event, { online: true })).toMatchObject({ step: "relay-socket", error: "Event:error", message: null });
  });

  it("says nothing it cannot know", () => {
    expect(dialFailureDetail(undefined, { online: null })).toEqual({
      step: null,
      error: null,
      message: null,
      online: null,
    });
  });
});
