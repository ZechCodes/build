import { describe, expect, it } from "vitest";
import { validateDeployRelay } from "../vite.config.js";

describe("deployable SPA build configuration", () => {
  it("rejects a versioned build that would fall back to the local relay", () => {
    expect(() => validateDeployRelay({ buildVersion: "abc123", relayUrl: undefined })).toThrow(
      "VITE_RELAY_URL is required",
    );
  });

  it.each(["ws://localhost:18090", "ws://127.0.0.1:18090", "ws://[::1]:18090"])(
    "rejects loopback relay %s for a versioned build",
    (relayUrl) => {
      expect(() => validateDeployRelay({ buildVersion: "abc123", relayUrl })).toThrow(
        "must not target localhost",
      );
    },
  );

  it("accepts the production relay for a versioned build", () => {
    expect(() =>
      validateDeployRelay({ buildVersion: "abc123", relayUrl: "wss://relay.getbuild.ing" }),
    ).not.toThrow();
  });

  it("keeps the zero-config local relay available to development builds", () => {
    expect(() => validateDeployRelay({ buildVersion: "dev", relayUrl: undefined })).not.toThrow();
  });
});
