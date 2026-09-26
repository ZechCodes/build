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

  // A limit set elsewhere — another client, the bridge's own tooling — is
  // none of the offered sizes, and still has to stand as the checked row.
  it("marks a limit set elsewhere on a row of its own, ahead of off", () => {
    const custom = { max_context_tokens: 250000, compact_at_tokens: 250000 };

    expect(labels(custom)).toEqual(["Default", "150k", "200k", "300k", "Custom (250k)", "Off"]);
    expect(marked(custom)).toEqual(["compact:250000"]);
    expect(compactionMenuOptions(custom)[4].description).toBe("Compact once a turn fills 250k tokens of context");
  });

  // The wire's limit is a u64; 2^53 is the first one Number.isSafeInteger
  // refuses, and past it the parse rounds — the row is checked against the
  // same Number the digest holds either way.
  it.each([2 ** 53, 2 ** 64, 1e21])("checks a custom row for a limit of %s and reads its id back", (limit) => {
    const options = compactionMenuOptions({ max_context_tokens: limit, compact_at_tokens: limit });

    expect(options.filter((option) => option.selected)).toEqual([expect.objectContaining({ id: `compact:${limit}` })]);
    expect(compactionLimitOfOptionId(`compact:${limit}`)).toEqual({ maxContextTokens: limit });
  });

  it("offers no custom row where the limit is one of the offered ones", () => {
    for (const max_context_tokens of [null, 150000, 200000, 300000, 0]) {
      expect(compactionMenuOptions({ max_context_tokens, compact_at_tokens: 200000 })).toHaveLength(5);
    }
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

  it("answers the limit a custom row stands for", () => {
    expect(compactionLimitOfOptionId("compact:250000")).toEqual({ maxContextTokens: 250000 });
  });

  it("answers null for a row that is not one of these", () => {
    expect(compactionLimitOfOptionId("detail:all")).toBe(null);
    expect(compactionLimitOfOptionId("compact:abc")).toBe(null);
    expect(compactionLimitOfOptionId("compact:-5")).toBe(null);
    expect(compactionLimitOfOptionId("compact:0")).toBe(null);
    expect(compactionLimitOfOptionId("compact:1.5")).toBe(null);
    expect(compactionLimitOfOptionId("compact:0x10")).toBe(null);
    expect(compactionLimitOfOptionId("compact: 5")).toBe(null);
    expect(compactionLimitOfOptionId("compact:")).toBe(null);
    expect(compactionLimitOfOptionId("compact:Infinity")).toBe(null);
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

describe("writing what was answered", () => {
  const AGENT = { id: "agent-2", max_context_tokens: null, compact_at_tokens: 200000 };
  const OFF = { agent_id: "agent-2", max_context_tokens: 0, compact_at_tokens: 0 };

  /** The row's write as a capture sees it, and a write that lays the rewrite
   *  over one held agent — the real cache's ordering is
   *  compactionAnswerCache.test.js's. */
  const cacheHolding = (agent) => {
    const cache = { agent, order: 1 };
    const steps = [];
    const capture = vi.fn(async (entityId) => {
      steps.push("capture");
      return { entityId, order: cache.order };
    });
    const write = vi.fn(async (captured, agentId, rewrite) => {
      steps.push("write");
      const next = rewrite(cache.agent);
      if (next) cache.agent = next;
    });
    const call = (answer) => vi.fn(async () => {
      steps.push("call");
      return answer;
    });
    return { cache, steps, capture, write, call };
  };

  it("takes the row's write before asking and offers the answer against it", async () => {
    const held = cacheHolding(AGENT);
    const choice = createCompactionChoice({ call: held.call(OFF), capture: held.capture, write: held.write });

    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 0 });

    expect(held.steps).toEqual(["capture", "call", "write"]);
    expect(held.write).toHaveBeenCalledWith({ entityId: "run-7", order: 1 }, "agent-2", expect.any(Function));
    expect(held.cache.agent).toEqual({ id: "agent-2", max_context_tokens: 0, compact_at_tokens: 0 });
  });

  it("leaves an agent that already says the answer alone", async () => {
    const pushed = { ...AGENT, max_context_tokens: 0, compact_at_tokens: 0 };
    const held = cacheHolding(pushed);
    const choice = createCompactionChoice({ call: held.call(OFF), capture: held.capture, write: held.write });

    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 0 });

    expect(held.cache.agent).toBe(pushed);
  });

  it("sends nothing for the row already standing, or while a choice is in flight", async () => {
    let release;
    const call = vi.fn(() => new Promise((resolve) => (release = resolve)));
    const held = cacheHolding(AGENT);
    const choice = createCompactionChoice({ call, capture: held.capture, write: held.write });

    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: null });
    const first = choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 150000 });
    await Promise.resolve();
    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 300000 });
    release({ max_context_tokens: 150000, compact_at_tokens: 150000 });
    await first;

    expect(call.mock.calls).toEqual([
      ["conversation.settings", { entity_id: "run-7", agent_id: "agent-2", max_context_tokens: 150000 }],
    ]);
  });

  it("hands a refusal to onFailure and writes nothing", async () => {
    const onFailure = vi.fn();
    const held = cacheHolding(AGENT);
    const choice = createCompactionChoice({
      call: async () => {
        throw new Error("not_found");
      },
      capture: held.capture,
      write: held.write,
      onFailure,
    });

    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 0 });

    expect(onFailure).toHaveBeenCalledTimes(1);
    expect(held.write).not.toHaveBeenCalled();
    expect(held.cache.agent).toBe(AGENT);
  });

  it("does not call a cache that cannot take the answer a refusal", async () => {
    const onFailure = vi.fn();
    const failing = async () => {
      throw new Error("cache unavailable");
    };
    const unread = createCompactionChoice({ call: async () => OFF, capture: failing, write: vi.fn(), onFailure });
    const unwritten = createCompactionChoice({
      call: async () => OFF,
      capture: async () => ({ order: 1 }),
      write: failing,
      onFailure,
    });

    await unread.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 0 });
    await unwritten.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 0 });

    expect(onFailure).not.toHaveBeenCalled();
  });
});
