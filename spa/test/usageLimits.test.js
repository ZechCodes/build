// @vitest-environment jsdom
// A harness out of usage on a device (issue #58): what the bridge's board says,
// the words the banner wears as it counts down, the banner itself at the top of
// a conversation, and the reason a Queued message gives on hover.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  applyQueuedReason,
  mountUsageLimitBanner,
  onUsageLimitsChanged,
  queuedReason,
  resetUsageLimits,
  setUsageLimits,
  usageLimitText,
  usageLimitsOf,
} from "../src/core/usageLimits.js";

const NOW = Date.parse("2026-09-20T21:46:00Z");
const SAID = "You've hit your session limit · resets 6:20pm (America/New_York)";
const limit = (overrides = {}) => ({
  harness: "claude_adk",
  since: "2026-09-20T21:30:47Z",
  resets_at: "2026-09-20T22:20:00Z",
  said: SAID,
  ...overrides,
});

beforeEach(() => {
  resetUsageLimits();
  document.body.innerHTML = "";
});

afterEach(() => {
  vi.useRealTimers();
});

describe("the banner's words", () => {
  it("says which harness, which limit, and how long until it resets", () => {
    expect(usageLimitText(limit(), NOW)).toBe("Claude session limit reached · resets in 34 min");
  });

  it("counts down at a few offsets, rounding a part minute up", () => {
    const at = (iso) => usageLimitText(limit(), Date.parse(iso));
    expect(at("2026-09-20T22:19:30Z")).toBe("Claude session limit reached · resets in 1 min");
    expect(at("2026-09-20T22:10:00Z")).toBe("Claude session limit reached · resets in 10 min");
    expect(at("2026-09-20T21:10:00Z")).toBe("Claude session limit reached · resets in 1 h 10 min");
    expect(at("2026-09-20T20:20:00Z")).toBe("Claude session limit reached · resets in 2 h");
    expect(at("2026-09-20T22:20:00Z")).toBe("Claude session limit reached · resets now");
  });

  it("says the reset time is unknown when the bridge could not resolve one", () => {
    expect(usageLimitText(limit({ resets_at: null }), NOW)).toBe(
      "Claude session limit reached · reset time unknown",
    );
  });

  it("names the limit the harness named", () => {
    const weekly = limit({
      said: "You've hit your weekly limit · resets Sep 25, 6pm (America/New_York)",
      resets_at: "2026-09-25T22:00:00Z",
    });
    expect(usageLimitText(weekly, NOW)).toBe("Claude weekly limit reached · resets in 5 d 1 h");
  });
});

describe("the device's limits", () => {
  it("are what the bridge last said, per device, and a change is announced once", () => {
    const heard = vi.fn();
    const stop = onUsageLimitsChanged(heard);
    setUsageLimits("dev-1", [limit()]);
    setUsageLimits("dev-1", [limit()]);
    expect(heard).toHaveBeenCalledTimes(1);
    expect(usageLimitsOf("dev-1")).toEqual([limit()]);
    expect(usageLimitsOf("dev-2")).toEqual([]);
    setUsageLimits("dev-1", []);
    expect(heard).toHaveBeenCalledTimes(2);
    expect(usageLimitsOf("dev-1")).toEqual([]);
    stop();
  });

  it("ignores a list the bridge did not send", () => {
    setUsageLimits("dev-1", [limit()]);
    setUsageLimits("dev-1", undefined);
    expect(usageLimitsOf("dev-1")).toEqual([limit()]);
  });
});

describe("the banner at the top of a conversation", () => {
  const panel = () => {
    document.body.innerHTML = `<div id="rail-panel"><div class="rail-head"></div><div class="rail-body" id="rail-body"></div></div>`;
    return document.querySelector("#rail-panel");
  };

  it("appears from the push, counts down each minute, and goes when the bridge clears it", () => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW);
    const host = panel();
    const banner = mountUsageLimitBanner(() => host, "dev-1");
    expect(host.querySelector(".usage-limit-banner")).toBeNull();

    setUsageLimits("dev-1", [limit()]);
    const shown = host.querySelector(".usage-limit-banner");
    expect(shown.textContent).toContain("Claude session limit reached · resets in 34 min");
    expect(host.querySelector(".usage-limit-banners").nextElementSibling.id).toBe("rail-body");

    vi.advanceTimersByTime(60_000);
    expect(host.querySelector(".usage-limit-text").textContent).toBe(
      "Claude session limit reached · resets in 33 min",
    );

    setUsageLimits("dev-1", []);
    expect(host.querySelector(".usage-limit-banners")).toBeNull();
    banner.dispose();
  });

  it("keeps the harness's own words behind a press", () => {
    setUsageLimits("dev-1", [limit()]);
    const host = panel();
    const banner = mountUsageLimitBanner(() => host, "dev-1");
    const details = host.querySelector("details.usage-limit-banner");
    expect(details.open).toBe(false);
    expect(details.querySelector(".usage-limit-said").textContent).toBe(SAID);
    details.querySelector("summary").click();
    expect(details.open).toBe(true);
    banner.sync();
    expect(host.querySelector("details.usage-limit-banner").open).toBe(true);
    banner.dispose();
  });

  it("shows another device's limit nowhere", () => {
    setUsageLimits("dev-2", [limit()]);
    const host = panel();
    const banner = mountUsageLimitBanner(() => host, "dev-1");
    expect(host.querySelector(".usage-limit-banner")).toBeNull();
    banner.dispose();
  });

  it("comes back after the panel is rebuilt under it", () => {
    setUsageLimits("dev-1", [limit()]);
    let host = panel();
    const banner = mountUsageLimitBanner(() => host, "dev-1");
    host = panel();
    expect(host.querySelector(".usage-limit-banner")).toBeNull();
    banner.sync();
    expect(host.querySelector(".usage-limit-banner")).not.toBeNull();
    banner.dispose();
    expect(host.querySelector(".usage-limit-banner")).toBeNull();
  });
});

describe("a Queued message's hover", () => {
  const chips = () => {
    document.body.innerHTML = `
      <span class="delivery-status queued" data-delivery-status="queued" aria-label="Queued for the agent">Queued</span>
      <span class="delivery-status submitted" data-delivery-status="submitted" aria-label="Queued for the agent">Queued</span>
      <span class="delivery-status sent" data-delivery-status="sent" aria-label="Sent to the agent">Sent</span>`;
    return document.body;
  };

  it("carries the reason while the conversation's harness is limited", () => {
    setUsageLimits("dev-1", [limit()]);
    const reason = queuedReason("dev-1", "claude_adk", NOW);
    expect(reason).toBe("Claude session limit reached · resets in 34 min");
    const body = chips();
    applyQueuedReason(body, reason);
    const [queued, submitted, sent] = body.querySelectorAll(".delivery-status");
    expect(queued.title).toBe(reason);
    expect(submitted.title).toBe(reason);
    expect(sent.title).toBe("");
  });

  it("carries none for another harness, and loses it when the limit clears", () => {
    setUsageLimits("dev-1", [limit()]);
    expect(queuedReason("dev-1", "codex_app_server", NOW)).toBeNull();
    const body = chips();
    applyQueuedReason(body, queuedReason("dev-1", "claude_adk", NOW));
    setUsageLimits("dev-1", []);
    applyQueuedReason(body, queuedReason("dev-1", "claude_adk", NOW));
    expect(body.querySelector(".queued").hasAttribute("title")).toBe(false);
  });
});
