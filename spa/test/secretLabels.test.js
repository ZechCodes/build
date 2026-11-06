import { expect, it } from "vitest";
import { secretChipLabel, secretTtlLabel } from "../src/core/secretLabels.js";

it("owns the strict metadata-only chip copy shared by staged and sent secrets", () => {
  expect(secretChipLabel({
    name: "DEPLOY_TOKEN",
    grant: { scope: "agent" },
    ttl_seconds: 3600,
  })).toBe("DEPLOY_TOKEN · Agent · 1 hour after sending");
  expect(secretChipLabel({ name: "MISSING_SCOPE", ttl_seconds: null })).toBe("");
  expect(secretTtlLabel(61)).toBe("61 seconds after sending");
});
