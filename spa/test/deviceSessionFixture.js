// A paired device's session, as the suites hand it to adoptDeviceSession.
//
// Adoption is what puts a machine's context on the registry, so a surface that
// asks a device for anything — its calls, its conversations, the harnesses it
// offers — has one to ask. The shape is minted here so no suite restates it.

import { vi } from "vitest";

/** A bridge that records everything asked of it and answers nothing. */
export const fakeSession = (deviceId) => ({
  deviceId,
  call: vi.fn(async () => ({})),
  close: vi.fn(),
  peer: vi.fn(),
  onCarrier: vi.fn(),
  onPush: vi.fn(),
});

/** A bridge that answers with whatever `app.call` is standing at the time: a
 *  suite that hands over a new one mid-test is that bridge answering
 *  differently, not another machine. */
export const sessionAnswering = (app, deviceId = "dev-1") => ({
  ...fakeSession(deviceId),
  call: (...args) => app.call(...args),
});
