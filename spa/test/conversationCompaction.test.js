// When a conversation compacts, as the ⋮ on its head offers it.
import { describe, expect, it, vi } from "vitest";

import {
  compactionMenuOptions,
  compactionLimitOfOptionId,
  compactionSettingsParams,
  createCompactionChoice,
} from "../src/core/conversationCompaction.js";

const labels = (agent) => compactionMenuOptions(agent).map((option) => option.label);
const marked = (agent) => compactionMenuOptions(agent).filter((option) => option.selected).map((option) => option.id);

describe("the compaction rows", () => {
  it("offers the device's default, three sizes and never", () => {
    expect(labels({ max_context_tokens: null, compact_at_tokens: 200000 })).toEqual([
      "Default (200k)",
      "150k",
      "200k",
      "300k",
      "Off",
    ]);
  });

  it("marks the default for a conversation with no limit of its own", () => {
    expect(marked({ max_context_tokens: null, compact_at_tokens: 200000 })).toEqual(["compact:default"]);
    // A digest from before the field reads the same way.
    expect(marked({ compact_at_tokens: 200000 })).toEqual(["compact:default"]);
  });

  it("marks the conversation's own limit, and off for zero", () => {
    expect(marked({ max_context_tokens: 150000, compact_at_tokens: 150000 })).toEqual(["compact:150000"]);
    expect(marked({ max_context_tokens: 300000, compact_at_tokens: 300000 })).toEqual(["compact:300000"]);
    expect(marked({ max_context_tokens: 0, compact_at_tokens: 0 })).toEqual(["compact:off"]);
  });

  it("names the default by the threshold in effect, which may be off", () => {
    expect(labels({ max_context_tokens: null, compact_at_tokens: 0 })[0]).toBe("Default (off)");
    expect(labels({ max_context_tokens: null, compact_at_tokens: 250000 })[0]).toBe("Default (250k)");
  });

  // With a limit of its own standing, the digest's threshold is that limit, not
  // the device's — so the default row cannot say what it would be.
  it("names the default without a number while an override stands", () => {
    expect(labels({ max_context_tokens: 150000, compact_at_tokens: 150000 })[0]).toBe("Default");
  });
});

describe("reading a row back", () => {
  it("answers the limit each row sends", () => {
    expect(
      ["compact:default", "compact:150000", "compact:200000", "compact:300000", "compact:off"].map((id) =>
        compactionLimitOfOptionId(id),
      ),
    ).toEqual([{ maxContextTokens: null }, { maxContextTokens: 150000 }, { maxContextTokens: 200000 }, { maxContextTokens: 300000 }, { maxContextTokens: 0 }]);
  });

  it("answers null for a row that is not one of these", () => {
    expect(compactionLimitOfOptionId("detail:all")).toBe(null);
    expect(compactionLimitOfOptionId("compact:123")).toBe(null);
    expect(compactionLimitOfOptionId(undefined)).toBe(null);
  });

  it("asks conversation.settings in the fixture's shape", () => {
    expect(compactionSettingsParams("run-7", "agent-2", 150000)).toEqual({
      entity_id: "run-7",
      agent_id: "agent-2",
      max_context_tokens: 150000,
    });
  });
});

describe("the hold on what was asked", () => {
  const AGENT = { id: "agent-2", max_context_tokens: null, compact_at_tokens: 200000 };

  it("shows what the bridge answered until the digest says the same", async () => {
    const call = async () => ({ agent_id: "agent-2", max_context_tokens: 0, compact_at_tokens: 0 });
    const choice = createCompactionChoice({ call });

    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 0 });

    expect(choice.agentAsKnown(AGENT)).toMatchObject({ max_context_tokens: 0, compact_at_tokens: 0 });
    const caughtUp = { ...AGENT, max_context_tokens: 0, compact_at_tokens: 0 };
    expect(choice.agentAsKnown(caughtUp)).toBe(caughtUp);
    // Agreed once, the digest is the truth again: a change made elsewhere shows.
    expect(choice.agentAsKnown(AGENT)).toBe(AGENT);
  });

  it("sends nothing for the row already standing, or while a choice is in flight", async () => {
    let release;
    const call = vi.fn(() => new Promise((resolve) => (release = resolve)));
    const choice = createCompactionChoice({ call });

    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: null });
    const first = choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 150000 });
    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 300000 });
    release({ max_context_tokens: 150000, compact_at_tokens: 150000 });
    await first;

    expect(call.mock.calls).toEqual([
      ["conversation.settings", { entity_id: "run-7", agent_id: "agent-2", max_context_tokens: 150000 }],
    ]);
  });

  it("hands a refusal to onFailure and holds nothing", async () => {
    const onFailure = vi.fn();
    const choice = createCompactionChoice({
      call: async () => {
        throw new Error("not_found");
      },
      onFailure,
    });

    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 0 });

    expect(onFailure).toHaveBeenCalledTimes(1);
    expect(choice.agentAsKnown(AGENT)).toBe(AGENT);
  });
});
