import { describe, it, expect } from "vitest";
import {
  allDevicesOfflineText,
  deviceOfflineMark,
  deviceOfflineText,
  deviceUnreachableText,
  esc,
  humanAge,
  messageOf,
  waitingForDeviceText,
} from "../src/core/text.js";

describe("esc", () => {
  it("escapes the HTML-significant characters and stringifies nullish", () => {
    expect(esc("<img src=x onerror=alert(1)>")).toBe("&lt;img src=x onerror=alert(1)&gt;");
    expect(esc("a & b")).toBe("a &amp; b");
    expect(esc(null)).toBe("");
    expect(esc(undefined)).toBe("");
  });

  it("escapes quotes so untrusted text cannot break out of an attribute value", () => {
    // esc() output is interpolated into attributes (data-dir="...", data-tab="...");
    // an unescaped quote would let a hostile filename inject live attributes.
    expect(esc('x" onmouseover="alert(1)')).toBe("x&quot; onmouseover=&quot;alert(1)");
    expect(esc("x' onmouseover='alert(1)")).toBe("x&#39; onmouseover=&#39;alert(1)");
  });
});

describe("messageOf", () => {
  it("shows the words a refusal carried, whatever it was thrown as", () => {
    // Every surface that prints a failed call goes through this, so a rejection
    // that is not an Error still has to read as something.
    expect(messageOf(new Error("unknown project_id: p2"))).toBe("unknown project_id: p2");
    expect(messageOf("the relay is offline")).toBe("the relay is offline");
    expect(messageOf({ code: 7 })).toBe("[object Object]");
  });
});

describe("waitingForDeviceText", () => {
  it("waits for the one machine the account has, or for any of the several", () => {
    // The screen lists every machine below the heading, so "your device" over a
    // list of three reads as a promise about one of them in particular.
    expect(waitingForDeviceText(1)).toBe("Waiting for your device");
    expect(waitingForDeviceText(3)).toBe("Waiting for a device");
  });
});

describe("humanAge", () => {
  it("renders human-scale ages across the four bands", () => {
    expect(humanAge(5)).toBe("just now");
    expect(humanAge(90)).toBe("1m ago");
    expect(humanAge(7200)).toBe("2h ago");
    expect(humanAge(259200)).toBe("3d ago");
  });

  it("clamps just under each boundary", () => {
    expect(humanAge(59)).toBe("just now");
    expect(humanAge(3599)).toBe("59m ago");
    expect(humanAge(86399)).toBe("23h ago");
  });
});

describe("deviceUnreachableText", () => {
  it("names the device, stamps the time it went unreachable, and promises resumption", () => {
    // A fixed local timestamp — assert on the pieces, not a locale-exact string.
    const sinceMs = new Date(2026, 6, 18, 15, 42).getTime();
    const text = deviceUnreachableText("Zech's MacBook", sinceMs);
    expect(text).toContain("Zech's MacBook");
    expect(text).toContain("unreachable since");
    expect(text).toContain("tasks will resume when it reconnects");
    // Carries a rendered clock time (locale-formatted h:mm).
    expect(text).toMatch(/\d{1,2}:\d{2}/);
  });

  // The fallback for an unnamed machine lives here with its sentence, the way
  // the other two device sentences keep theirs.
  it("says whose device it is when this client cannot name it", () => {
    expect(deviceUnreachableText(null, Date.now())).toMatch(/^Your device unreachable since /);
  });
});

describe("allDevicesOfflineText", () => {
  it("says every device is offline, in plain words", () => {
    // With nothing reachable there is no device to name and no time that means
    // anything: it says what is true and what happens next.
    const text = allDevicesOfflineText();
    expect(text).toContain("All devices are offline");
    expect(text).toContain("tasks will resume when one reconnects");
  });
});

describe("deviceOfflineText", () => {
  it("names the device that cannot be reached and says what that means here", () => {
    // The one sentence a work surface prints in place of itself, whether the
    // machine its link names has gone offline or was never opened here.
    const text = deviceOfflineText("Zech's MacBook");
    expect(text).toContain("Zech's MacBook");
    expect(text).toContain("isn't connected");
  });

  it("falls back to plain words when the account has no name for the device", () => {
    expect(deviceOfflineText(null)).toContain("That device");
  });
});

describe("deviceOfflineMark", () => {
  it("is the short label a control whose machine cannot answer wears", () => {
    // A label, not a sentence: it is a greyed row's title and a shut menu
    // item's reason, and it lives here with the account's other offline words
    // rather than as a literal inside the module that paints them.
    expect(deviceOfflineMark).toBe("Device offline");
  });
});
