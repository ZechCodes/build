import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../src/core/bridgeApi/v1/index.js";
import { commandRefusalMessage, notifyCommandFailure } from "../src/core/commandRefusal.js";
import { notifyError } from "../src/core/notify.js";

vi.mock("../src/core/notify.js", () => ({ notifyError: vi.fn() }));
beforeEach(() => vi.clearAllMocks());

const unsupported = "This bridge does not support watching tasks yet.";

it("shows unsupported commands in the visible notice summary", () => {
  notifyCommandFailure(new ApiError("unknown_method", "unknown method: tasks.watch"), "Could not change watching", unsupported);
  expect(notifyError).toHaveBeenCalledWith(unsupported);
});

it("keeps other errors as details under the action summary", () => {
  notifyCommandFailure(new Error("The device is unavailable."), "Could not change watching", unsupported);
  expect(notifyError).toHaveBeenCalledWith("Could not change watching", "The device is unavailable.");
});

describe("command refusal copy", () => {
  it.each([
    new ApiError("unknown_method", "unknown method: tasks.watch"),
    Object.assign(new Error("unrecognized command"), { error_code: "unknown_method" }),
    new Error("unknown method: tasks.watch"),
    new Error("Method not found"),
    "unknown method: tasks.watch",
  ])("explains an older bridge's refusal in the surface's words: %s", (error) => {
    expect(commandRefusalMessage(error, unsupported)).toBe(unsupported);
  });

  it.each([
    new ApiError("permission_denied", "You cannot change this task."),
    new Error("The device is unavailable."),
    "The file is too large.",
  ])("preserves other refusal details: %s", (error) => {
    expect(commandRefusalMessage(error, unsupported)).toBe(error.message || error);
  });
});
