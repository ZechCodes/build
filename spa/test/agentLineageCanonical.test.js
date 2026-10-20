import { describe, expect, it } from "vitest";
import { agentLineage, buildAgentEntries, lineagePrint } from "../src/core/agentLineageModel.js";

const member = (agent, entityId = "project-owner") => ({ agent, entityId, kind: "project", workspaceId: null, workspaceName: "" });
const pin = { id: "pin", conversation_id: "pin-history", working: false };
const worker = { id: "worker-a", conversation_id: "worker-history", created_by: pin.id, working: true };

describe("canonical conversations in agent lineage (#697 review)", () => {
  it.each([false, true])("counts and lists one spawned child (exact repeat: %s)", (repeated) => {
    const rows = [member(pin), member(worker), member({ ...worker, id: repeated ? worker.id : "worker-b" })];
    const lineage = agentLineage(rows);
    expect.soft(lineage.rollup(pin.id).agentsRunning).toBe(1);
    expect.soft(buildAgentEntries(lineage, pin.id)).toHaveLength(1);
  });

  it("preserves descendants naming a discarded creator alias and accepts both creator ids", () => {
    const old = { id: "creator-old", conversation_id: "creator-history", working: false };
    const latest = { ...old, id: "creator-new" };
    const rows = [member(old), member(latest), member({ ...worker, created_by: old.id }, "workspace-owner")];
    const held = structuredClone(rows);
    const lineage = agentLineage(rows);
    expect.soft(lineage.rollup(old.id).agentsRunning).toBe(1);
    expect.soft(lineage.rollup(latest.id).agentsRunning).toBe(1);
    expect.soft(buildAgentEntries(lineage, latest.id)).toHaveLength(1);
    expect(rows).toEqual(held);
  });

  it("uses the last canonical digest for every alias's own working state", () => {
    const old = { id: "old", conversation_id: "same-history", working: true };
    const latest = { ...old, id: "new", working: false };
    const lineage = agentLineage([member(old), member(latest)]);
    expect(lineage.rollup(old.id).running).toBe(false);
    expect(lineage.rollup(latest.id).running).toBe(false);
  });

  it("cuts a creator cycle introduced when aliases collapse", () => {
    const old = { id: "old", conversation_id: "same-history", created_by: "new", working: false };
    const latest = { ...old, id: "new", created_by: "old", working: true };
    const lineage = agentLineage([member(old), member(latest)]);
    expect(lineage.rollup(old.id)).toMatchObject({ running: true, agentsRunning: 0 });
    expect(lineage.createdBy(old.id)).toEqual([]);
  });

  it("keeps the same conversation string on different owners distinct", () => {
    const lineage = agentLineage([member(pin), member(worker), member({ ...worker, id: "worker-b" }, "other-owner")]);
    expect(lineage.rollup(pin.id).agentsRunning).toBe(2);
  });

  it("invalidates cached lineage when only canonical conversation membership changes", () => {
    expect(lineagePrint([member(worker)])).not.toBe(lineagePrint([member({ ...worker, conversation_id: "new-history" })]));
  });
});
