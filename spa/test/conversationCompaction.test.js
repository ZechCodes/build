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
  const OFF = { agent_id: "agent-2", max_context_tokens: 0, compact_at_tokens: 0 };

  /** One agent in a cache: `write` lays the rewrite over it, as the rail's
   *  row record does, and leaves it alone where the rewrite answers null. */
  const cacheHolding = (agent) => {
    const cache = { agent };
    const write = vi.fn(async (entityId, agentId, rewrite) => {
      const next = rewrite(cache.agent);
      if (next) cache.agent = next;
    });
    return { cache, write };
  };

  it("writes what the bridge answered into the cached agent", async () => {
    const { cache, write } = cacheHolding(AGENT);
    const choice = createCompactionChoice({ call: async () => OFF, write });

    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 0 });

    expect(write).toHaveBeenCalledWith("run-7", "agent-2", expect.any(Function));
    expect(cache.agent).toMatchObject({ id: "agent-2", max_context_tokens: 0, compact_at_tokens: 0 });
  });

  it("writes the answer back over a digest from before it", async () => {
    const { cache, write } = cacheHolding(AGENT);
    const choice = createCompactionChoice({ call: async () => OFF, write });
    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 0 });
    // The rail takes up its own write: that is not the bridge agreeing.
    choice.takeUp([cache.agent]);

    // A push built before the change lands after the reply.
    cache.agent = AGENT;
    choice.takeUp([AGENT]);
    await Promise.resolve();

    expect(cache.agent).toMatchObject({ max_context_tokens: 0, compact_at_tokens: 0 });
  });

  it("lets the digest go once the bridge's own word agrees, so a change made elsewhere shows", async () => {
    const { cache, write } = cacheHolding(AGENT);
    const choice = createCompactionChoice({ call: async () => OFF, write });
    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 0 });

    const pushed = { ...AGENT, max_context_tokens: 0, compact_at_tokens: 0 };
    cache.agent = pushed;
    choice.takeUp([pushed]);
    const elsewhere = { ...AGENT, max_context_tokens: 150000, compact_at_tokens: 150000 };
    cache.agent = elsewhere;
    choice.takeUp([elsewhere]);
    await Promise.resolve();

    expect(write).toHaveBeenCalledTimes(1);
    expect(cache.agent).toBe(elsewhere);
  });

  it("writes nothing where the push agreeing beat the reply to the cache", async () => {
    const pushed = { ...AGENT, max_context_tokens: 0, compact_at_tokens: 0 };
    const { cache, write } = cacheHolding(pushed);
    const choice = createCompactionChoice({ call: async () => OFF, write });

    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 0 });
    const elsewhere = { ...AGENT, max_context_tokens: 300000, compact_at_tokens: 300000 };
    cache.agent = elsewhere;
    choice.takeUp([elsewhere]);
    await Promise.resolve();

    expect(write).toHaveBeenCalledTimes(1);
    expect(cache.agent).toBe(elsewhere);
  });

  it("sends nothing for the row already standing, or while a choice is in flight", async () => {
    let release;
    const call = vi.fn(() => new Promise((resolve) => (release = resolve)));
    const choice = createCompactionChoice({ call, write: cacheHolding(AGENT).write });

    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: null });
    const first = choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 150000 });
    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 300000 });
    release({ max_context_tokens: 150000, compact_at_tokens: 150000 });
    await first;

    expect(call.mock.calls).toEqual([
      ["conversation.settings", { entity_id: "run-7", agent_id: "agent-2", max_context_tokens: 150000 }],
    ]);
  });

  it("hands a refusal to onFailure and writes nothing", async () => {
    const onFailure = vi.fn();
    const { cache, write } = cacheHolding(AGENT);
    const choice = createCompactionChoice({
      call: async () => {
        throw new Error("not_found");
      },
      write,
      onFailure,
    });

    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 0 });
    choice.takeUp([AGENT]);

    expect(onFailure).toHaveBeenCalledTimes(1);
    expect(write).not.toHaveBeenCalled();
    expect(cache.agent).toBe(AGENT);
  });

  it("does not call a cache that cannot take the answer a refusal", async () => {
    const onFailure = vi.fn();
    const choice = createCompactionChoice({
      call: async () => OFF,
      write: async () => {
        throw new Error("cache unavailable");
      },
      onFailure,
    });

    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 0 });

    expect(onFailure).not.toHaveBeenCalled();
  });
});
