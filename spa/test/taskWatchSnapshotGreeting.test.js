// UNITCONTRACT identity controls; no task response or stored snapshot can greet.
import { expect, it } from "vitest";
import { createBridgeEpoch } from "../src/core/bridgeEpoch.js";

const greeting = (journal = "journal-A") => ({ api_version: "3.27.0",
  capabilities: ["bridge.epoch", "tasks.watchSnapshot.get.v1"],
  bridge_epoch: "same-process", journal_incarnation: journal });

it("never establishes the snapshot incarnation from an unexpected reply even when it names both identities", () => {
  const epoch = createBridgeEpoch();
  expect(epoch.accepts({ ...greeting(), watch_snapshot: { version: 1, sequence: "18446744073709551615" } })).toBe(false);
  expect(epoch.current()).toBeNull();
});

it("does not let a superseded greeting install a snapshot incarnation", () => {
  const epoch = createBridgeEpoch();
  epoch.acceptGreeting(greeting());
  const held = epoch.current();
  expect(epoch.acceptGreeting(greeting("retired-journal"), { isCurrent: () => false })).toBe(false);
  expect(epoch.current()).toBe(held);
});
