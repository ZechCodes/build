import { describe, expect, it } from "vitest";
import { selectDeviceTransport } from "../src/core/deviceTransportSelection.js";

const personal = "https://127.0.0.1:8443";
const hosted = "https://127.0.0.1:9443";
const projection = (fields = {}) => ({
  source: "webrtc", rtc_allowed: true, reason: null, origin: null, assignment_revision: null, ...fields,
});
const device = (transport) => ({ id: "dev", fingerprint: "pin", transport });
const manual = { deviceId: "dev", origin: personal, selected: "http" };

describe("selection from the committed device record", () => {
  it("keeps legacy and free/manual choices", () => {
    expect(selectDeviceTransport(device(), null, false).kind).toBe("webrtc");
    expect(selectDeviceTransport(device(), manual, false)).toMatchObject({ kind: "http", origin: personal, wssOnly: false });
    const free = device(projection({ source: "personal", origin: personal }));
    expect(selectDeviceTransport(free, null, true).kind).toBe("webrtc");
    expect(selectDeviceTransport(free, manual, true)).toMatchObject({ kind: "http", origin: personal, wssOnly: false });
  });

  it("chooses hosted or policy-selected personal HTTPS with no RTC terminal fallback", () => {
    const assigned = device(projection({ source: "hosted", origin: hosted, assignment_revision: 1 }));
    expect(selectDeviceTransport(assigned, manual, true)).toMatchObject({ kind: "http", origin: hosted, wssOnly: true });
    const registered = device(projection({ source: "personal", origin: personal, rtc_allowed: false }));
    expect(selectDeviceTransport(registered, null, true)).toMatchObject({ kind: "http", origin: personal, wssOnly: true });
  });

  it("honors affirmative bootstrap permission and holds with the cached reason after it ends", () => {
    const pending = projection({ reason: "setup_pending" });
    expect(selectDeviceTransport(device(pending), manual, true).kind).toBe("webrtc");
    const disallowed = device({ ...pending, rtc_allowed: false });
    expect(selectDeviceTransport(disallowed, manual, true)).toMatchObject({ kind: "blocked", reason: "setup_pending" });
    expect(selectDeviceTransport(device(), manual, true)).toMatchObject({ kind: "blocked", reason: "policy_unknown" });
  });

  it("does not use an obsolete origin when the issuer withdraws or rotates the assignment", () => {
    const ready = projection({ source: "hosted", rtc_allowed: false, origin: hosted, assignment_revision: 1 });
    const first = selectDeviceTransport(device(ready), null, true);
    expect(selectDeviceTransport(device({ ...ready, assignment_revision: 2 }), null, true).key).not.toBe(first.key);
    expect(selectDeviceTransport(device({ ...ready, origin: null }), manual, true).kind).toBe("blocked");
  });
});
