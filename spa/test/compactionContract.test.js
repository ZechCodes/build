import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import * as v1 from "../src/core/bridgeApi/v1/index.js";

const fixture = (name) => JSON.parse(readFileSync(new URL(`../../fixtures/api/v1/${name}.json`, import.meta.url), "utf8"));
const providers = ["claude", "claude_adk", "codex", "codex_app_server", "pi"];

describe("compaction wire fixtures", () => {
  it("announces the feature, setters and invalidation in the additive minor", () => {
    const hello = fixture("session.hello").result;
    expect(hello.capabilities).toContain("compaction.harnessThresholds");
    expect(v1.capabilitiesOf(hello).compaction.harnessThresholds).toBe(true);
    for (const method of ["project.set_compaction", "workspace.set_compaction"]) {
      expect(hello.capabilities).toContain(method);
      expect(fixture(method).since).toBe("3.22.0");
      expect(v1.parseResult(method, fixture(method).result)).toEqual(fixture(method).result);
    }
    const events = fixture("events").events.filter(({ type }) => type === "compaction.changed");
    expect(events.map(({ scope }) => scope)).toEqual(["device", "project", "workspace", "conversation"]);
    for (const event of events) {
      expect(Object.keys(event).sort()).toEqual(["bridge_epoch", "owner_id", "scope", "type"]);
      expect(event.bridge_epoch).toBe(hello.bridge_epoch);
      expect(v1.parseEvent(event)).not.toBeNull();
    }
  });

  it("ships inherited and effective observations for every provider at every scope", () => {
    const records = [fixture("settings.get").result, fixture("settings.set").result,
      fixture("project.list").result.projects[0], fixture("workspace.get").result,
      fixture("workspace.list").result.workspaces[0], fixture("project.set_compaction").result,
      fixture("workspace.set_compaction").result];
    for (const record of records) {
      expect(record.compaction_thresholds).toBeTypeOf("object");
      for (const part of ["inherited", "effective"]) {
        expect(Object.keys(record.compaction[part]).sort()).toEqual(providers);
        for (const observation of Object.values(record.compaction[part])) {
          expect(observation.source.kind).toBeTypeOf("string");
          expect(observation.tokens).toBeGreaterThan(0);
        }
      }
    }
  });

  it("keeps the conversation override separate from inheritance and unknown active policy", () => {
    const result = fixture("conversation.settings").result;
    expect(result.max_context_tokens).toBe(150000);
    expect(result.compaction.inherited.tokens).toBe(200000);
    const agents = fixture("agent.list").result.agents;
    expect(agents.length).toBeGreaterThan(0);
    for (const agent of agents) {
      expect(agent.compaction.configured.tokens).toBe(agent.compact_at_tokens);
      expect(agent.compaction.inherited.tokens).toBeGreaterThan(0);
      expect(agent.compaction.active).toBeNull();
      expect(agent.compaction.enforcement).toBe("unsupported");
      expect(agent.compaction.pending_change).toBeNull();
    }
  });

  it("retains exact unsafe legacy values only as read observations", () => {
    const example = fixture("settings.get").examples.find(({ result }) => result.compaction_thresholds.codex?.tokens_decimal);
    expect(example.result.compaction_thresholds.codex).toEqual({ tokens_decimal: "18446744073709551615" });
    expect(example.result.compaction.effective.codex).toMatchObject({
      tokens: null, tokens_decimal: "18446744073709551615", reason: "unsupported_representation",
    });
    expect(v1.parseResult("settings.get", example.result)).toEqual(example.result);
  });

  it("represents a saved scope policy pending over the applied snapshot", () => {
    const example = fixture("agent.list").examples[0];
    const agent = example.result.agents[1];
    expect(agent.compact_at_tokens).toBe(200000);
    expect(agent.compaction.configured.tokens).toBe(150000);
    expect(agent.compaction.active).toMatchObject({ tokens: 200000, evidence: "applied_at_start" });
    expect(agent.compaction.pending_change).toBe(true);
    expect(v1.parseResult("agent.list", example.result)).toEqual(example.result);
  });
});


it("explains missing compaction support with the agreed editor copy", async () => {
  const adapter = v1.create(() => { throw new Error("must not send"); }, { api_version: "3.24.0", capabilities: [] });
  await expect(adapter.call("settings.set", { compaction_thresholds: { codex: 150000 } }))
    .rejects.toMatchObject({ code: "unknown_method", message: "Update the bridge to set compaction defaults by harness." });
});
