// `buildConnectionProbe()`: whether each machine's session is carried right
// now, asked over the path it rides. The gate's ICE restart check sends it
// after a restart, because an idle path carries nothing on its own (#131).

import { describe, expect, it, vi } from "vitest";
import { PROBE_TIMEOUT_MS, probeConnections } from "../src/core/connectionProbe.js";

const machine = (deviceId, confirmCarried) => ({ deviceId, session: confirmCarried && { confirmCarried } });

describe("the connection probe", () => {
  it("asks every machine's session once, and says which carried", async () => {
    const carried = vi.fn(async () => true);
    const silent = vi.fn(async () => false);
    const rows = await probeConnections(1234, [machine("dev-a", carried), machine("dev-b", silent)]);
    expect(rows.map(({ deviceId, carried: yes }) => [deviceId, yes])).toEqual([
      ["dev-a", true],
      ["dev-b", false],
    ]);
    expect(carried).toHaveBeenCalledOnce();
    expect(carried).toHaveBeenCalledWith(1234);
    expect(silent).toHaveBeenCalledWith(1234);
    expect(rows.every((row) => Number.isFinite(row.ms) && row.ms >= 0)).toBe(true);
  });

  it("counts only a plain yes, and a machine with nothing to ask as not carried", async () => {
    const rows = await probeConnections(undefined, [machine("dev-a", async () => "maybe"), machine("dev-b", null)]);
    expect(rows.map((row) => row.carried)).toEqual([false, false]);
  });

  it("gives each machine the default time when not told otherwise", async () => {
    const ask = vi.fn(async () => true);
    await probeConnections(undefined, [machine("dev-a", ask)]);
    expect(ask).toHaveBeenCalledWith(PROBE_TIMEOUT_MS);
  });
});
