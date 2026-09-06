// Who hears a channel close.
//
// "The two channels are one connection and fall back together" is written in
// connection.js, which registers on both halves and hands both streams back at
// once. The manager is the setter it hands the terminal's half to — a second
// listener here would run the same fallback twice, through two owners of one
// fact.

import { describe, it, expect, vi } from "vitest";

vi.mock("@build/secure-transport", () => ({ ready: async () => {} }));
vi.mock("../src/config.js", () => ({ RELAY_URL: "wss://relay.test" }));
vi.mock("../src/app.js", () => ({ App: {} }));
vi.mock("../src/api.js", () => ({ fetchGatewayToken: async () => "tok" }));
vi.mock("../src/devices.js", () => ({ pinnedDeviceTransportKey: async () => "pk" }));

const { terminalsRideOn } = await import("../src/terminal/manager.js");

describe("terminalsRideOn", () => {
  it("watches nothing on the carrier it is handed", () => {
    const carrier = { onClose: vi.fn(), onEnvelope: vi.fn(), send: vi.fn(), close: vi.fn() };

    terminalsRideOn(carrier);

    expect(carrier.onClose).not.toHaveBeenCalled();
    terminalsRideOn(null);
  });
});
